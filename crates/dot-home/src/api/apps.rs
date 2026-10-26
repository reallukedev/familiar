//! Apps in a Mac: which ones the assistant may use, installing more, custom packs.
use super::owned_instance;
use crate::{
    auth::Session,
    error::{ApiError, ApiResult},
    packs::{Pack, Risk},
    state::App,
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use dot_wire::{AppInfo, AppsOp, InstallSource, Op};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

fn tools_json(p: &Pack) -> Vec<Value> {
    p.tools
        .iter()
        .map(|t| json!({ "name": t.name, "title": t.title, "risk": t.risk }))
        .collect()
}

/// Installed apps: live from the Mac when it's up (and cached), else the cache.
async fn installed(app: &App, id: &str) -> Vec<AppInfo> {
    let machine = app.fleet.machine(id);
    if let Some(link) = machine.link()
        && let Ok(list) = link
            .call_as::<Vec<AppInfo>>(
                Op::Apps(AppsOp::List {
                    running_only: false,
                }),
                Duration::from_secs(20),
            )
            .await
    {
        let (iid, json) = (
            id.to_owned(),
            serde_json::to_string(&list).unwrap_or_default(),
        );
        let _ = app
            .db
            .call(move |c| {
                c.execute(
                    "UPDATE instances SET apps_cache = ?2 WHERE id = ?1",
                    [iid, json],
                )
            })
            .await;
        return list;
    }
    let iid = id.to_owned();
    app.db
        .call(move |c| {
            c.query_row(
                "SELECT apps_cache FROM instances WHERE id = ?1",
                [iid],
                |r| r.get::<_, Option<String>>(0),
            )
        })
        .await
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

async fn switches(app: &App, id: &str) -> ApiResult<Vec<(String, bool)>> {
    let iid = id.to_owned();
    Ok(app
        .db
        .call(move |c| {
            let mut s =
                c.prepare("SELECT pack_id, enabled FROM enabled_packs WHERE instance_id = ?1")?;
            let rows = s.query_map([iid], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
        .await?)
}

async fn entries(app: &App, id: &str) -> ApiResult<Vec<Value>> {
    let apps = installed(app, id).await;
    let switches = switches(app, id).await?;
    let on = |key: &str| {
        switches
            .iter()
            .find(|(k, _)| k == key)
            .is_none_or(|(_, e)| *e)
    };
    let iid = id.to_owned();
    let custom: Vec<String> = app
        .db
        .call(move |c| {
            let mut s = c.prepare("SELECT manifest FROM custom_packs WHERE instance_id = ?1")?;
            let rows = s.query_map([iid], |r| r.get(0))?;
            rows.collect()
        })
        .await?;
    let mut out = Vec::new();
    let mut covered = std::collections::HashSet::new();
    let mut pack_entry = |p: &Pack, kind: &str| {
        let info = p.bundle_id.as_ref().and_then(|b| {
            apps.iter()
                .find(|a| a.bundle_id.as_deref() == Some(b.as_str()))
        });
        if let Some(b) = &p.bundle_id {
            covered.insert(b.clone());
        }
        json!({
            "id": p.id, "name": p.name, "bundle_id": p.bundle_id,
            "installed": info.is_some() || apps.is_empty(), "running": info.is_some_and(|a| a.running),
            "version": info.and_then(|a| a.version.clone()), "scriptable": true,
            "enabled": on(&p.id), "kind": kind, "tools": tools_json(p), "icon": null
        })
    };
    for p in app.catalog.sorted() {
        out.push(pack_entry(p, "builtin"));
    }
    for m in custom {
        if let Ok(p) = Pack::parse(&m) {
            out.push(pack_entry(&p, "custom"));
        }
    }
    for a in &apps {
        let Some(bundle) = &a.bundle_id else { continue };
        if covered.contains(bundle) || bundle.starts_with("com.apple.") && !a.scriptable {
            continue;
        }
        let key = format!("app:{bundle}");
        out.push(json!({
            "id": bundle, "name": a.name, "bundle_id": bundle, "installed": true, "running": a.running,
            "version": a.version, "scriptable": a.scriptable, "enabled": on(&key),
            "kind": if a.scriptable { "scriptable" } else { "ui_only" }, "tools": [], "icon": null
        }));
    }
    Ok(out)
}

/// `GET /api/instances/:id/apps`.
pub async fn list(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<Value>>> {
    let id = owned_instance(&app, &s, &id).await?;
    Ok(Json(entries(&app, &id).await?))
}

#[derive(Deserialize)]
pub struct EnableBody {
    enabled: bool,
}

/// `PUT /api/instances/:id/apps/:app`.
pub async fn set_enabled(
    State(app): State<App>,
    s: Session,
    Path((id, app_id)): Path<(String, String)>,
    Json(b): Json<EnableBody>,
) -> ApiResult<Json<Value>> {
    let id = owned_instance(&app, &s, &id).await?;
    let all = entries(&app, &id).await?;
    let entry = all
        .iter()
        .find(|e| e["id"] == app_id.as_str())
        .ok_or_else(|| ApiError::not_found("app"))?;
    let key = if matches!(entry["kind"].as_str(), Some("builtin" | "custom")) {
        app_id.clone()
    } else {
        format!("app:{app_id}")
    };
    let iid = id.clone();
    app.db
        .call(move |c| {
            c.execute(
                "INSERT INTO enabled_packs (instance_id, pack_id, enabled) VALUES (?1, ?2, ?3)
                 ON CONFLICT(instance_id, pack_id) DO UPDATE SET enabled = excluded.enabled",
                rusqlite::params![iid, key, b.enabled],
            )
        })
        .await?;
    let mut updated = entry.clone();
    updated["enabled"] = json!(b.enabled);
    Ok(Json(updated))
}

#[derive(Deserialize)]
pub struct InstallBody {
    source: InstallSource,
}

/// `POST /api/instances/:id/apps/install`. Answers within ~45 s; longer installs finish
/// in the background (`202` with `installing: true`).
pub async fn install(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
    Json(b): Json<InstallBody>,
) -> ApiResult<Response> {
    let id = owned_instance(&app, &s, &id).await?;
    if let InstallSource::Url { url, sha256 } = &b.source {
        if !url.starts_with("https://") || url::Url::parse(url).is_err() {
            return Err(ApiError::invalid(
                "url",
                "Use an https link to a .dmg, .zip or .pkg.",
            ));
        }
        if let Some(h) = sha256
            && (h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()))
        {
            return Err(ApiError::invalid(
                "sha256",
                "A SHA-256 is 64 hexadecimal characters.",
            ));
        }
    }
    if let InstallSource::Brew { cask } = &b.source
        && (cask.is_empty()
            || cask.len() > 100
            || !cask
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '@' | '.' | '/')))
    {
        return Err(ApiError::invalid(
            "cask",
            "That isn't a Homebrew cask name.",
        ));
    }
    let machine = app.fleet.machine(&id);
    let link = machine
        .ensure_ready(Duration::from_secs(40))
        .await
        .map_err(|e| ApiError::unavailable(format!("{e}. Start the Mac, then install.")))?;
    let task = tokio::spawn(async move {
        link.call_as::<AppInfo>(
            Op::Apps(AppsOp::Install { source: b.source }),
            Duration::from_secs(900),
        )
        .await
    });
    match tokio::time::timeout(Duration::from_secs(45), task).await {
        Ok(Ok(Ok(info))) => Ok(Json(json!({
            "id": info.bundle_id.clone().unwrap_or_else(|| info.name.clone()), "name": info.name, "bundle_id": info.bundle_id,
            "installed": true, "running": info.running, "version": info.version, "scriptable": info.scriptable,
            "enabled": true, "kind": if info.scriptable { "scriptable" } else { "ui_only" }, "tools": [], "icon": null
        }))
        .into_response()),
        Ok(Ok(Err(e))) => Err(ApiError::bad(format!("The install didn't work: {e}"))),
        Ok(Err(_)) => Err(ApiError::internal("install task stopped")),
        Err(_) => Ok((StatusCode::ACCEPTED, Json(json!({ "installing": true }))).into_response()),
    }
}

#[derive(Deserialize)]
pub struct CustomBody {
    manifest: String,
}

/// `POST /api/instances/:id/apps/custom`.
pub async fn custom_add(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
    Json(b): Json<CustomBody>,
) -> ApiResult<Json<Value>> {
    let id = owned_instance(&app, &s, &id).await?;
    let pack = Pack::parse(&b.manifest).map_err(|e| ApiError::invalid(e.field, e.message))?;
    if app.catalog.packs.contains_key(&pack.id) {
        return Err(ApiError::invalid(
            "id",
            "A built-in pack already uses that id. Choose another.",
        ));
    }
    if let Some(t) = pack
        .tools
        .iter()
        .find(|t| crate::tools::catalog::builtin(&t.name).is_some())
    {
        return Err(ApiError::invalid(
            "tools",
            format!(
                "\u{201c}{}\u{201d} is a built-in tool name. Choose another.",
                t.name
            ),
        ));
    }
    let builtin_names: std::collections::HashSet<&str> = app
        .catalog
        .packs
        .values()
        .flat_map(|p| p.tools.iter().map(|t| t.name.as_str()))
        .collect();
    if let Some(t) = pack
        .tools
        .iter()
        .find(|t| builtin_names.contains(t.name.as_str()))
    {
        return Err(ApiError::invalid(
            "tools",
            format!(
                "\u{201c}{}\u{201d} is already used by a built-in pack.",
                t.name
            ),
        ));
    }
    let (iid, pid, manifest) = (id.clone(), pack.id.clone(), b.manifest.clone());
    let count = app
        .db
        .call(move |c| {
            let n: i64 = c.query_row("SELECT COUNT(*) FROM custom_packs WHERE instance_id = ?1", [&iid], |r| r.get(0))?;
            if n >= 50 {
                return Ok(n);
            }
            c.execute(
                "INSERT INTO custom_packs (instance_id, pack_id, manifest, created_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(instance_id, pack_id) DO UPDATE SET manifest = excluded.manifest",
                rusqlite::params![iid, pid, manifest, crate::db::now()],
            )?;
            Ok(n)
        })
        .await?;
    if count >= 50 {
        return Err(ApiError::budget("A Mac can have up to 50 custom packs."));
    }
    let sends = pack.tools.iter().any(|t| t.risk == Risk::Send);
    Ok(Json(json!({
        "id": pack.id, "name": pack.name, "bundle_id": pack.bundle_id, "installed": true, "running": false,
        "version": null, "scriptable": true, "enabled": true, "kind": "custom", "tools": tools_json(&pack), "icon": null,
        "sends": sends
    })))
}

/// `DELETE /api/instances/:id/apps/custom/:app`.
pub async fn custom_remove(
    State(app): State<App>,
    s: Session,
    Path((id, pack)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let id = owned_instance(&app, &s, &id).await?;
    let n = app
        .db
        .call(move |c| {
            c.execute(
                "DELETE FROM custom_packs WHERE instance_id = ?1 AND pack_id = ?2",
                [id, pack],
            )
        })
        .await?;
    if n == 0 {
        return Err(ApiError::not_found("custom pack"));
    }
    Ok(Json(json!({})))
}
