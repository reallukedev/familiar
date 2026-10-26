//! Admin: host health, people, invites, every Mac's resource use (never its contents),
//! server limits and the macOS image builder.
use super::instances::delete_instance;
use crate::{
    auth::Session,
    crypto,
    db::{now, rfc3339, rfc3339_opt},
    error::{ApiError, ApiResult},
    host,
    state::App,
};
use axum::{
    Json,
    extract::{Path, State},
};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

/// `GET /api/admin/overview`.
pub async fn overview(State(app): State<App>, s: Session) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    Ok(Json(overview_json(&app).await?))
}

async fn overview_json(app: &App) -> ApiResult<Value> {
    let (mem_total, mem_avail) = host::memory().unwrap_or((0, 0));
    let (disk_total, disk_free) = host::disk(&app.config.data_dir).unwrap_or((0, 0));
    let (used_mem, running) = app.fleet.usage(None).await;
    let budget_mem = app
        .db
        .setting("memory_budget_mb")
        .await?
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| app.fleet.memory_budget_mb());
    let max_running: u32 = app
        .db
        .setting("max_running")
        .await?
        .and_then(|v| v.parse().ok())
        .unwrap_or(app.config.limits.max_running);
    let (users, instances): (i64, i64) = app
        .db
        .call(|c| c.query_row("SELECT (SELECT COUNT(*) FROM users), (SELECT COUNT(*) FROM instances WHERE is_builder = 0)", [], |r| Ok((r.get(0)?, r.get(1)?))))
        .await?;
    Ok(json!({
        "hostname": std::fs::read_to_string("/etc/hostname").map(|h| h.trim().to_owned()).unwrap_or_else(|_| "server".into()),
        "version": crate::updates::build::version(),
        "uptime_s": host::uptime().unwrap_or_else(|| app.started.elapsed().as_secs()),
        "cpu_count": std::thread::available_parallelism().map_or(0, std::num::NonZero::get),
        "load_1m": host::load().unwrap_or(0.0),
        "memory_total_mb": mem_total, "memory_available_mb": mem_avail,
        "disk_total_bytes": disk_total, "disk_free_bytes": disk_free,
        "budget": { "memory_mb": budget_mem, "max_running": max_running, "used_memory_mb": used_mem, "running": running },
        "checks": host::checks(&app.config, app.fleet.reserve_gb().await).await,
        "require_totp": app.require_totp.load(Ordering::Relaxed),
        "users": users, "instances": instances
    }))
}

/// `GET /api/admin/users`.
pub async fn users(State(app): State<App>, s: Session) -> ApiResult<Json<Vec<Value>>> {
    s.require_admin()?;
    let default_max = app.config.limits.max_instances_per_user;
    Ok(Json(
        app.db
            .call(move |c| {
                let mut st = c.prepare(
                    "SELECT u.id, u.username, u.display_name, u.role, u.totp_secret IS NOT NULL, u.created_at, u.disabled,
                            u.max_instances, (SELECT COUNT(*) FROM instances i WHERE i.user_id = u.id AND i.is_builder = 0), u.last_seen_at
                     FROM users u ORDER BY u.created_at",
                )?;
                let rows = st.query_map([], |r| {
                    Ok(json!({
                        "id": r.get::<_, String>(0)?, "username": r.get::<_, String>(1)?, "display_name": r.get::<_, String>(2)?,
                        "role": r.get::<_, String>(3)?, "totp_enabled": r.get::<_, bool>(4)?, "created_at": rfc3339(r.get(5)?),
                        "disabled": r.get::<_, bool>(6)?, "max_instances": r.get::<_, Option<i64>>(7)?.unwrap_or(i64::from(default_max)),
                        "instance_count": r.get::<_, i64>(8)?, "last_seen_at": rfc3339_opt(r.get(9)?)
                    }))
                })?;
                rows.collect()
            })
            .await?,
    ))
}

#[derive(Deserialize)]
pub struct UserPatch {
    disabled: Option<bool>,
    role: Option<String>,
    max_instances: Option<i64>,
}

/// `PATCH /api/admin/users/:id`.
pub async fn user_patch(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
    Json(b): Json<UserPatch>,
) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    s.require_confirmed()?;
    if id == s.user.id && (b.disabled == Some(true) || b.role.as_deref() == Some("member")) {
        return Err(ApiError::forbidden(
            "You can't turn off or demote your own account. Ask another admin.",
        ));
    }
    if let Some(r) = &b.role
        && !matches!(r.as_str(), "admin" | "member")
    {
        return Err(ApiError::invalid("role", "Choose admin or member."));
    }
    if let Some(m) = b.max_instances
        && !(0..=50).contains(&m)
    {
        return Err(ApiError::invalid("max_instances", "Choose 0 to 50."));
    }
    let uid = id.clone();
    let n = app
        .db
        .call(move |c| {
            let tx = c.transaction()?;
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM users WHERE id = ?1)",
                [&uid],
                |r| r.get(0),
            )?;
            if !exists {
                return Ok(0);
            }
            if let Some(d) = b.disabled {
                tx.execute(
                    "UPDATE users SET disabled = ?2 WHERE id = ?1",
                    rusqlite::params![uid, d],
                )?;
                if d {
                    tx.execute("DELETE FROM sessions WHERE user_id = ?1", [&uid])?;
                }
            }
            if let Some(r) = &b.role {
                tx.execute(
                    "UPDATE users SET role = ?2 WHERE id = ?1",
                    rusqlite::params![uid, r],
                )?;
            }
            if let Some(m) = b.max_instances {
                tx.execute(
                    "UPDATE users SET max_instances = ?2 WHERE id = ?1",
                    rusqlite::params![uid, m],
                )?;
            }
            tx.commit()?;
            Ok(1)
        })
        .await?;
    if n == 0 {
        return Err(ApiError::not_found("person"));
    }
    let all = users(State(app), s).await?.0;
    all.into_iter()
        .find(|u| u["id"] == id)
        .map(Json)
        .ok_or_else(|| ApiError::not_found("person"))
}

#[derive(Deserialize)]
pub struct ConfirmBody {
    confirm: String,
}

/// `DELETE /api/admin/users/:id`: deletes their Macs too.
pub async fn user_delete(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
    Json(b): Json<ConfirmBody>,
) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    s.require_confirmed()?;
    if id == s.user.id {
        return Err(ApiError::forbidden(
            "You can't delete your own account here.",
        ));
    }
    let uid = id.clone();
    let username: String = app
        .db
        .call(move |c| {
            c.query_row("SELECT username FROM users WHERE id = ?1", [uid], |r| {
                r.get(0)
            })
            .optional()
        })
        .await?
        .ok_or_else(|| ApiError::not_found("person"))?;
    if b.confirm.trim() != username {
        return Err(ApiError::invalid(
            "confirm",
            "Type their username exactly to delete them.",
        ));
    }
    let uid = id.clone();
    let instances: Vec<String> = app
        .db
        .call(move |c| {
            let mut st = c.prepare("SELECT id FROM instances WHERE user_id = ?1")?;
            let rows = st.query_map([uid], |r| r.get(0))?;
            rows.collect()
        })
        .await?;
    for i in instances {
        delete_instance(&app, &i).await?;
    }
    let uid = id.clone();
    app.db
        .call(move |c| c.execute("DELETE FROM users WHERE id = ?1", [uid]))
        .await?;
    app.fleet.keys().remove(&id);
    Ok(Json(json!({})))
}

async fn new_invite(
    app: &App,
    s: &Session,
    role: &str,
    reset_user: Option<String>,
    hours: i64,
) -> ApiResult<(Value, String)> {
    let code = crypto::random_id::<24>();
    let hash = crypto::sha256(code.as_bytes());
    let id = crypto::random_id::<8>();
    let t = now();
    let (iid, role_s, by) = (id.clone(), role.to_owned(), s.user.id.clone());
    app.db
        .call(move |c| {
            if let Some(u) = &reset_user {
                // One live reset link per person.
                c.execute("DELETE FROM invites WHERE reset_user = ?1 AND used_at IS NULL", [u])?;
            }
            c.execute(
                "INSERT INTO invites (id, code_hash, role, reset_user, created_by, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![iid, hash, role_s, reset_user, by, t, t + hours * 3600],
            )
        })
        .await?;
    let url = format!("{}/invite/{code}", app.config.origin());
    Ok((
        json!({ "id": id, "role": role, "created_at": rfc3339(t), "expires_at": rfc3339(t + hours * 3600), "used": false }),
        url,
    ))
}

/// `POST /api/admin/users/:id/reset`: a one-time link to set a new password.
pub async fn user_reset(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    s.require_confirmed()?;
    let uid = id.clone();
    let role: String = app
        .db
        .call(move |c| {
            c.query_row("SELECT role FROM users WHERE id = ?1", [uid], |r| r.get(0))
                .optional()
        })
        .await?
        .ok_or_else(|| ApiError::not_found("person"))?;
    let (_, url) = new_invite(&app, &s, &role, Some(id.clone()), 24).await?;
    tracing::info!(user = %id, admin = %s.user.username, "password reset link created");
    Ok(Json(json!({ "invite_url": url })))
}

/// `GET /api/admin/instances`: resource use only.
pub async fn instances(State(app): State<App>, s: Session) -> ApiResult<Json<Vec<Value>>> {
    s.require_admin()?;
    let rows: Vec<(String, String, String, u32, u32, Option<i64>)> = app
        .db
        .call(|c| {
            let mut st = c.prepare(
                "SELECT i.id, i.name, u.username, i.memory_mb, i.cpus, i.last_active_at FROM instances i JOIN users u ON u.id = i.user_id
                 WHERE i.is_builder = 0 ORDER BY u.username, i.created_at",
            )?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?;
            rows.collect()
        })
        .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, name, owner, mem, cpus, last)| {
                let state = app.fleet.machine(&id).status().state;
                json!({ "id": id, "name": name, "owner": owner, "state": state, "memory_mb": mem, "cpus": cpus, "last_active_at": rfc3339_opt(last) })
            })
            .collect(),
    ))
}

/// `POST /api/admin/instances/:id/stop`.
pub async fn instance_stop(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    let iid = id.clone();
    let exists: bool = app
        .db
        .call(move |c| {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM instances WHERE id = ?1)",
                [iid],
                |r| r.get(0),
            )
        })
        .await?;
    if !exists {
        return Err(ApiError::not_found("Mac"));
    }
    app.fleet
        .machine(&id)
        .stop(false)
        .await
        .map_err(ApiError::unavailable)?;
    tracing::info!(instance = %id, admin = %s.user.username, "stopped by admin");
    Ok(Json(json!({})))
}

/// `GET /api/admin/invites`.
pub async fn invites(State(app): State<App>, s: Session) -> ApiResult<Json<Vec<Value>>> {
    s.require_admin()?;
    Ok(Json(
        app.db
            .call(|c| {
                let mut st = c.prepare("SELECT id, role, created_at, expires_at, used_at FROM invites WHERE reset_user IS NULL ORDER BY created_at DESC LIMIT 100")?;
                let rows = st.query_map([], |r| {
                    Ok(json!({ "id": r.get::<_, String>(0)?, "role": r.get::<_, String>(1)?, "created_at": rfc3339(r.get(2)?),
                               "expires_at": rfc3339(r.get(3)?), "used": r.get::<_, Option<i64>>(4)?.is_some() }))
                })?;
                rows.collect()
            })
            .await?,
    ))
}

#[derive(Deserialize)]
pub struct InviteBody {
    role: String,
    expires_in_hours: Option<i64>,
}

/// `POST /api/admin/invites`.
pub async fn invite_create(
    State(app): State<App>,
    s: Session,
    Json(b): Json<InviteBody>,
) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    s.require_confirmed()?;
    if !matches!(b.role.as_str(), "admin" | "member") {
        return Err(ApiError::invalid("role", "Choose admin or member."));
    }
    let hours = b.expires_in_hours.unwrap_or(72);
    if !(1..=720).contains(&hours) {
        return Err(ApiError::invalid(
            "expires_in_hours",
            "Choose 1 hour to 30 days.",
        ));
    }
    let (invite, url) = new_invite(&app, &s, &b.role, None, hours).await?;
    Ok(Json(json!({ "invite": invite, "url": url })))
}

/// `DELETE /api/admin/invites/:id`.
pub async fn invite_delete(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    app.db
        .call(move |c| c.execute("DELETE FROM invites WHERE id = ?1", [id]))
        .await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub struct SettingsBody {
    require_totp: Option<bool>,
    memory_budget_mb: Option<u64>,
    max_running: Option<u32>,
}

/// `PUT /api/admin/settings`.
pub async fn settings(
    State(app): State<App>,
    s: Session,
    Json(b): Json<SettingsBody>,
) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    s.require_confirmed()?;
    if let Some(v) = b.require_totp {
        if v && !s.user.totp_enabled {
            return Err(ApiError::invalid(
                "require_totp",
                "Turn on two-factor authentication for your own account first.",
            ));
        }
        app.db.set_setting("require_totp", v.to_string()).await?;
        app.require_totp.store(v, Ordering::Relaxed);
    }
    if let Some(v) = b.memory_budget_mb {
        let total = host::memory().map_or(u64::MAX, |(t, _)| t);
        if v < 4096 || v > total {
            return Err(ApiError::invalid(
                "memory_budget_mb",
                "Choose at least 4 GB and no more than the server has.",
            ));
        }
        app.db
            .set_setting("memory_budget_mb", v.to_string())
            .await?;
    }
    if let Some(v) = b.max_running {
        if !(1..=64).contains(&v) {
            return Err(ApiError::invalid("max_running", "Choose 1 to 64."));
        }
        app.db.set_setting("max_running", v.to_string()).await?;
    }
    Ok(Json(overview_json(&app).await?))
}

pub use crate::images::{builder_delete, builder_promote, builder_start, images};
