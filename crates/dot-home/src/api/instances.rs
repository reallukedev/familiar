//! Macs: create, settings, power, live events, permissions, approvals, activity,
//! checkpoints and connections.
use super::{InstanceView, instance_view, owned_instance, publish_instance};
use crate::{
    auth::{self, Session},
    crypto,
    db::{now, rfc3339, rfc3339_opt},
    error::{ApiError, ApiResult},
    events::Audience,
    state::App,
    tools::{self, appmcp, approvals, catalog},
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response, sse},
};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

/// `GET /api/instances`.
pub async fn list(State(app): State<App>, s: Session) -> ApiResult<Json<Vec<InstanceView>>> {
    let uid = s.user.id.clone();
    let ids: Vec<String> = app
        .db
        .call(move |c| {
            let mut st = c.prepare("SELECT id FROM instances WHERE user_id = ?1 AND is_builder = 0 ORDER BY created_at")?;
            let rows = st.query_map([uid], |r| r.get(0))?;
            rows.collect()
        })
        .await?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push(instance_view(&app, &id).await?);
    }
    Ok(Json(out))
}

/// `GET /api/instances/:id`.
pub async fn get(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
) -> ApiResult<Json<InstanceView>> {
    let id = owned_instance(&app, &s, &id).await?;
    Ok(Json(instance_view(&app, &id).await?))
}

#[derive(Deserialize)]
pub struct CreateBody {
    name: String,
    #[serde(default = "qemu")]
    driver: String,
    cpus: Option<u32>,
    memory_mb: Option<u32>,
    disk_gb: Option<u32>,
    remote_host: Option<String>,
    /// `ask`, `changes` or `all`; Ask Before Changes when left out.
    approval_mode: Option<String>,
}

fn qemu() -> String {
    "qemu".into()
}

fn check_resources(cpus: u32, memory_mb: u32, disk_gb: u32) -> ApiResult<()> {
    let host_cpus = std::thread::available_parallelism().map_or(64, |n| n.get() as u32);
    if !(2..=host_cpus.max(2)).contains(&cpus) {
        return Err(ApiError::invalid(
            "cpus",
            format!("Choose 2 to {} processor cores.", host_cpus.max(2)),
        ));
    }
    if !(4096..=262_144).contains(&memory_mb) {
        return Err(ApiError::invalid(
            "memory_mb",
            "Choose 4 GB to 256 GB of memory.",
        ));
    }
    if !(64..=4096).contains(&disk_gb) {
        return Err(ApiError::invalid(
            "disk_gb",
            "Choose 64 GB to 4 TB of disk.",
        ));
    }
    Ok(())
}

/// Remote Macs must be on a private network; the guest link authenticates but doesn't encrypt.
fn check_remote(host: &str) -> ApiResult<()> {
    let bad = || {
        ApiError::invalid(
            "remote_host",
            "Enter the Mac's private address and port, like 100.64.12.3:7466 (Tailscale) or 192.168.1.20:7466.",
        )
    };
    let addr: std::net::SocketAddr = host.parse().map_err(|_| bad())?;
    let private = match addr.ip() {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_private() || v4.is_loopback() || (o[0] == 100 && (64..=127).contains(&o[1]))
        }
        std::net::IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00,
    };
    if private { Ok(()) } else { Err(bad()) }
}

/// `POST /api/instances`.
pub async fn create(
    State(app): State<App>,
    s: Session,
    Json(b): Json<CreateBody>,
) -> ApiResult<Json<Value>> {
    let name = crate::util::clean_name(&b.name, 40)
        .ok_or_else(|| ApiError::invalid("name", "Name the Mac (up to 40 characters)."))?;
    let vm = &app.config.vm;
    let (cpus, memory_mb, disk_gb) = (
        b.cpus.unwrap_or(vm.default_cpus),
        b.memory_mb.unwrap_or(vm.default_memory_mb),
        b.disk_gb.unwrap_or(vm.default_disk_gb),
    );
    match b.driver.as_str() {
        "qemu" => {
            check_resources(cpus, memory_mb, disk_gb)?;
            if !app.config.data_dir.join("images/base.qcow2").exists() {
                return Err(ApiError::unavailable(
                    "This server doesn't have a macOS image yet. An admin needs to build one in Admin > macOS Image.",
                ));
            }
            crate::storage::check_new_mac(&app, &s.user.id, u64::from(disk_gb)).await?;
        }
        "remote" => check_remote(b.remote_host.as_deref().unwrap_or(""))?,
        _ => {
            return Err(ApiError::invalid(
                "driver",
                "Choose a Mac on this server or a remote Mac.",
            ));
        }
    }
    let mode = match b.approval_mode.as_deref() {
        None => tools::modes::DEFAULT,
        Some(m) => *tools::modes::MODES
            .iter()
            .find(|x| **x == m)
            .ok_or_else(|| {
                ApiError::invalid(
                    "approval_mode",
                    "Choose Ask for Everything, Ask Before Changes or Allow Everything.",
                )
            })?,
    };
    let is_admin = s.user.is_admin();
    let default_max = i64::from(app.config.limits.max_instances_per_user);
    let id = crypto::random_id::<10>();
    let secret = dot_wire::random_bytes::<32>();
    let sealed = app.key.seal(&format!("guest:{id}"), &secret);
    let port_base = app.config.vm.port_base;
    let (iid, uid, driver, remote) = (
        id.clone(),
        s.user.id.clone(),
        b.driver.clone(),
        b.remote_host.clone(),
    );
    let desired = if driver == "remote" {
        "running"
    } else {
        "stopped"
    };
    let created = app
        .db
        .call(move |c| {
            let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            // Counted inside the write transaction so parallel creates can't pass the limit.
            let (count, max): (i64, Option<i64>) = tx.query_row(
                "SELECT (SELECT COUNT(*) FROM instances WHERE user_id = ?1 AND is_builder = 0), max_instances FROM users WHERE id = ?1",
                [&uid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let max = max.unwrap_or(default_max);
            if !is_admin && count >= max {
                return Ok(Err(max));
            }
            let port: Option<u16> = if driver == "qemu" {
                let used: Vec<u16> = {
                    let mut st = tx.prepare("SELECT port FROM instances WHERE port IS NOT NULL")?;
                    st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
                };
                (port_base..port_base.saturating_add(1000)).find(|p| !used.contains(p))
            } else {
                None
            };
            // Macs on this server are encrypted with their owner's key from the start.
            let encrypted = driver == "qemu";
            tx.execute(
                "INSERT INTO instances (id, user_id, name, driver, cpus, memory_mb, disk_gb, port, remote_host, secret, desired, created_at, approval_mode, encrypted)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                rusqlite::params![iid, uid, name, driver, cpus, memory_mb, disk_gb, port, remote, sealed, desired, now(), mode, encrypted],
            )?;
            tx.commit()?;
            Ok(Ok(()))
        })
        .await?;
    if let Err(max) = created {
        return Err(ApiError::budget(format!(
            "You can have up to {max} Mac{}. Delete one or ask your admin for more.",
            if max == 1 { "" } else { "s" }
        )));
    }
    app.fleet.machine(&id);
    // Matches `dot-guest pair`: dotpair:v1:<instance id>:<secret, base64url>:<port>.
    let pairing = (b.driver == "remote").then(|| {
        use base64::Engine;
        let port = b
            .remote_host
            .as_deref()
            .and_then(|h| h.parse::<std::net::SocketAddr>().ok())
            .map_or(dot_wire::GUEST_PORT, |a| a.port());
        format!(
            "dotpair:v1:{id}:{}:{port}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret)
        )
    });
    Ok(Json(
        json!({ "instance": instance_view(&app, &id).await?, "pairing": pairing }),
    ))
}

#[derive(Deserialize)]
pub struct PatchBody {
    name: Option<String>,
    paused: Option<bool>,
    cpus: Option<u32>,
    memory_mb: Option<u32>,
    sleep_after_minutes: Option<i64>,
    hibernate_after_minutes: Option<i64>,
    agent_channel: Option<String>,
    snapshot_keep_days: Option<i64>,
    snapshot_max_gb: Option<u64>,
}

/// `PATCH /api/instances/:id`.
pub async fn patch(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
    Json(b): Json<PatchBody>,
) -> ApiResult<Json<InstanceView>> {
    let id = owned_instance(&app, &s, &id).await?;
    let name = match &b.name {
        Some(n) => Some(
            crate::util::clean_name(n, 40)
                .ok_or_else(|| ApiError::invalid("name", "Name the Mac (up to 40 characters)."))?,
        ),
        None => None,
    };
    if b.cpus.is_some() || b.memory_mb.is_some() {
        let current = instance_view(&app, &id).await?;
        check_resources(
            b.cpus.unwrap_or(current_cpus(&current)),
            b.memory_mb.unwrap_or(current_mem(&current)),
            64,
        )?;
    }
    for (field, v) in [
        ("sleep_after_minutes", b.sleep_after_minutes),
        ("hibernate_after_minutes", b.hibernate_after_minutes),
    ] {
        if let Some(v) = v
            && !(0..=10_080).contains(&v)
        {
            return Err(ApiError::invalid(
                if field == "sleep_after_minutes" {
                    "sleep_after_minutes"
                } else {
                    "hibernate_after_minutes"
                },
                "Choose 0 (never) to 10080 minutes.",
            ));
        }
    }
    if let Some(d) = b.snapshot_keep_days
        && !(0..=crate::snapshots::MAX_KEEP_DAYS).contains(&d)
    {
        return Err(ApiError::invalid(
            "snapshot_keep_days",
            "Keep snapshots for 1 to 3650 days, or 0 for forever.",
        ));
    }
    if let Some(gb) = b.snapshot_max_gb
        && gb > 100_000
    {
        return Err(ApiError::invalid(
            "snapshot_max_gb",
            "Choose up to 100000 GB, or 0 for no limit.",
        ));
    }
    let retention_changed = b.snapshot_keep_days.is_some() || b.snapshot_max_gb.is_some();
    if let Some(c) = &b.agent_channel
        && !matches!(c.as_str(), "default" | "stable" | "nightly" | "off")
    {
        return Err(ApiError::invalid(
            "agent_channel",
            "Choose Use Server Setting, Stable, Nightly or Off.",
        ));
    }
    let iid = id.clone();
    app.db
        .call(move |c| {
            let tx = c.transaction()?;
            if let Some(ch) = &b.agent_channel {
                tx.execute(
                    "UPDATE instances SET agent_channel = ?2 WHERE id = ?1",
                    rusqlite::params![iid, ch],
                )?;
            }
            if let Some(n) = name {
                tx.execute(
                    "UPDATE instances SET name = ?2 WHERE id = ?1",
                    rusqlite::params![iid, n],
                )?;
            }
            if let Some(p) = b.paused {
                tx.execute(
                    "UPDATE instances SET paused = ?2 WHERE id = ?1",
                    rusqlite::params![iid, p],
                )?;
            }
            if let Some(v) = b.cpus {
                tx.execute(
                    "UPDATE instances SET cpus = ?2 WHERE id = ?1",
                    rusqlite::params![iid, v],
                )?;
            }
            if let Some(v) = b.memory_mb {
                tx.execute(
                    "UPDATE instances SET memory_mb = ?2 WHERE id = ?1",
                    rusqlite::params![iid, v],
                )?;
            }
            if let Some(v) = b.sleep_after_minutes {
                tx.execute(
                    "UPDATE instances SET sleep_after_minutes = ?2 WHERE id = ?1",
                    rusqlite::params![iid, v],
                )?;
            }
            if let Some(v) = b.hibernate_after_minutes {
                tx.execute(
                    "UPDATE instances SET hibernate_after_minutes = ?2 WHERE id = ?1",
                    rusqlite::params![iid, v],
                )?;
            }
            if let Some(v) = b.snapshot_keep_days {
                tx.execute(
                    "UPDATE instances SET snapshot_keep_days = ?2 WHERE id = ?1",
                    rusqlite::params![iid, v],
                )?;
            }
            if let Some(v) = b.snapshot_max_gb {
                tx.execute(
                    "UPDATE instances SET snapshot_max_gb = ?2 WHERE id = ?1",
                    rusqlite::params![iid, v],
                )?;
            }
            tx.commit()
        })
        .await?;
    app.fleet.machine(&id).reload().await;
    if retention_changed {
        // A shorter retention or a smaller cap applies now, not at the next sweep.
        let app = app.clone();
        tokio::spawn(async move { crate::snapshots::sweep(&app).await });
    }
    publish_instance(&app, &id).await;
    Ok(Json(instance_view(&app, &id).await?))
}

fn current_cpus(v: &InstanceView) -> u32 {
    serde_json::to_value(v)
        .ok()
        .and_then(|j| j["cpus"].as_u64())
        .unwrap_or(4) as u32
}
fn current_mem(v: &InstanceView) -> u32 {
    serde_json::to_value(v)
        .ok()
        .and_then(|j| j["memory_mb"].as_u64())
        .unwrap_or(8192) as u32
}

#[derive(Deserialize)]
pub struct ConfirmName {
    confirm: String,
}

/// `DELETE /api/instances/:id`: stop, remove files, forget.
pub async fn remove(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
    Json(b): Json<ConfirmName>,
) -> ApiResult<Json<Value>> {
    s.require_confirmed()?;
    let id = owned_instance(&app, &s, &id).await?;
    let iid = id.clone();
    let name: String = app
        .db
        .call(move |c| {
            c.query_row("SELECT name FROM instances WHERE id = ?1", [iid], |r| {
                r.get(0)
            })
        })
        .await?;
    if b.confirm.trim() != name {
        return Err(ApiError::invalid(
            "confirm",
            "Type the Mac's name exactly to delete it.",
        ));
    }
    delete_instance(&app, &id).await?;
    Ok(Json(json!({})))
}

/// Stop a Mac whose rows are already gone and delete its files (a reset that erased
/// encrypted Macs). Failures are logged; nothing references the files any more.
pub async fn remove_files(app: &App, id: &str) {
    let machine = app.fleet.machine(id);
    if let Err(e) = machine.stop(true).await {
        tracing::warn!(instance = %id, error = %e, "couldn't stop an erased Mac");
    }
    app.fleet.forget(id).await;
    let dir = app.fleet.files(id).dir;
    if dir.exists()
        && let Err(e) = tokio::fs::remove_dir_all(&dir).await
    {
        tracing::warn!(instance = %id, error = %e, "couldn't remove an erased Mac's files");
    }
    app.storage.forget(id);
}

/// Stop a Mac, delete its files and rows. Shared with admin user deletion.
pub async fn delete_instance(app: &App, id: &str) -> ApiResult<()> {
    let machine = app.fleet.machine(id);
    machine.stop(true).await.map_err(ApiError::unavailable)?;
    app.fleet.forget(id).await;
    let dir = app.fleet.files(id).dir;
    if dir.exists() {
        tokio::fs::remove_dir_all(&dir)
            .await
            .map_err(|e| ApiError::internal(format!("removing {}: {e}", dir.display())))?;
    }
    let iid = id.to_owned();
    app.db
        .call(move |c| c.execute("DELETE FROM instances WHERE id = ?1", [iid]))
        .await?;
    Ok(())
}

#[derive(Deserialize)]
pub struct PowerBody {
    action: String,
}

/// `POST /api/instances/:id/power`.
pub async fn power(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
    Json(b): Json<PowerBody>,
) -> ApiResult<Json<InstanceView>> {
    let id = owned_instance(&app, &s, &id).await?;
    let m = app.fleet.machine(&id);
    // A locked Mac can keep running, but can't come back from off, hibernation or a
    // restart until its owner unlocks it.
    let off = !m.status().state.uses_memory();
    if m.is_locked()
        && (b.action == "restart" || (off && matches!(b.action.as_str(), "start" | "wake")))
    {
        return Err(ApiError::locked());
    }
    let result = match b.action.as_str() {
        "start" => m.start().await,
        "stop" => m.stop(false).await,
        "force_stop" => m.stop(true).await,
        "restart" => m.restart().await,
        "sleep" => m.sleep().await,
        "wake" => {
            m.wake().await;
            Ok(())
        }
        _ => return Err(ApiError::invalid("action", "Unknown power action.")),
    };
    if let Err(e) = result {
        return Err(if e == crate::vm::LOCKED {
            ApiError::locked()
        } else if e.contains("limit") || e.contains("memory") {
            ApiError::budget(e)
        } else {
            ApiError::unavailable(e)
        });
    }
    publish_instance(&app, &id).await;
    Ok(Json(instance_view(&app, &id).await?))
}

/// `POST /api/instances/:id/encrypt`: convert a stopped Mac made before encryption.
/// Answers at once; progress arrives as state events.
pub async fn encrypt(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
) -> ApiResult<Json<InstanceView>> {
    let id = owned_instance(&app, &s, &id).await?;
    let view = instance_view(&app, &id).await?;
    if view.encrypted {
        return Err(ApiError::bad("This Mac is already encrypted."));
    }
    if !view.can_encrypt {
        return Err(ApiError::bad("Only Macs on this server can be encrypted."));
    }
    if !app.fleet.keys().is_unlocked(&s.user.id) {
        return Err(ApiError::locked());
    }
    let m = app.fleet.machine(&id);
    m.encrypt().await.map_err(|e| {
        if e == crate::vm::LOCKED {
            ApiError::locked()
        } else if e.starts_with("Shut down") {
            ApiError::conflict(e)
        } else {
            ApiError::bad(e)
        }
    })?;
    app.storage.forget(&id);
    publish_instance(&app, &id).await;
    Ok(Json(instance_view(&app, &id).await?))
}

/// `GET /api/instances/:id/thumbnail`: a small JPEG, cached briefly.
pub async fn thumbnail(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let id = owned_instance(&app, &s, &id).await?;
    let m = app.fleet.machine(&id);
    let Some(link) = m.link() else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    // One capture serves every open tab for a few seconds (the lock also coalesces bursts).
    let mut cache = m.thumbnail.lock().await;
    let fresh = cache
        .as_ref()
        .is_some_and(|(at, _, _)| at.elapsed() < Duration::from_secs(4));
    if !fresh {
        let shot: dot_wire::Screenshot = link
            .call_as(
                dot_wire::Op::Screenshot(dot_wire::ScreenshotArgs {
                    max_width: Some(640),
                    ..Default::default()
                }),
                Duration::from_secs(8),
            )
            .await
            .map_err(|e| ApiError::unavailable(e.to_string()))?;
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(shot.data)
            .map_err(ApiError::internal)?;
        *cache = Some((std::time::Instant::now(), bytes, shot.mime));
    }
    let Some((_, bytes, mime)) = cache.as_ref() else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    Ok((
        [
            (header::CONTENT_TYPE, mime.clone()),
            (header::CACHE_CONTROL, "private, max-age=4".to_owned()),
        ],
        bytes.clone(),
    )
        .into_response())
}

/// `GET /api/instances/:id/events`: live state, activity and approvals.
pub async fn events(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let id = owned_instance(&app, &s, &id).await?;
    let mut rx = app.events.subscribe();
    let is_admin = s.user.is_admin();
    let app2 = app.clone();
    let session_key = s.id_hash.clone();
    let mut recheck = tokio::time::interval(Duration::from_secs(30));
    recheck.tick().await;
    let stream = async_stream::stream! {
        if let Ok(v) = instance_view(&app2, &id).await {
            yield Ok::<_, std::convert::Infallible>(sse::Event::default().event("instance").json_data(v).unwrap_or_default());
        }
        loop {
            let next = tokio::select! {
                ev = rx.recv() => ev,
                _ = recheck.tick() => {
                    // End the stream once the session is gone (signed out, password changed, disabled).
                    if crate::auth::still_valid(&app2, &session_key).await { continue; }
                    break;
                }
            };
            match next {
                Ok(ev) => {
                    let mine = match &ev.audience {
                        Audience::Instance(i) => *i == id,
                        Audience::Admins => is_admin,
                    };
                    if !mine { continue; }
                    if ev.kind == "state" {
                        // State changes are sent as the full instance so the portal has one shape.
                        if let Ok(v) = instance_view(&app2, &id).await {
                            yield Ok(sse::Event::default().event("instance").json_data(v).unwrap_or_default());
                        }
                    } else {
                        yield Ok(sse::Event::default().event(ev.kind).json_data(ev.data).unwrap_or_default());
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    if let Ok(v) = instance_view(&app2, &id).await {
                        yield Ok(sse::Event::default().event("instance").json_data(v).unwrap_or_default());
                    }
                }
                Err(_) => break,
            }
        }
    };
    Ok(sse::Sse::new(stream)
        .keep_alive(
            sse::KeepAlive::new()
                .interval(Duration::from_secs(20))
                .event(sse::Event::default().event("ping").data("{}")),
        )
        .into_response())
}

/// `GET /api/instances/:id/permissions`.
pub async fn permissions(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<Value>>> {
    let id = owned_instance(&app, &s, &id).await?;
    Ok(Json(groups(&app, &id).await?))
}

pub(super) async fn groups(app: &App, id: &str) -> ApiResult<Vec<Value>> {
    let modes = tools::modes(app, id).await?;
    let mut out = Vec::new();
    for g in catalog::GROUPS {
        let mut tools_in: Vec<Value> = catalog::builtins()
            .iter()
            .filter(|t| t.group == g.id)
            .map(|t| json!({ "name": t.name, "title": t.title, "risk": t.risk }))
            .collect();
        let packs = tools::enabled_packs(app, id).await?;
        if g.id == "outgoing" {
            for p in &packs {
                tools_in.extend(
                    p.tools
                        .iter()
                        .filter(|t| t.risk == crate::packs::Risk::Send)
                        .map(|t| json!({ "name": t.name, "title": t.title, "risk": t.risk })),
                );
            }
        }
        out.push(json!({
            "id": g.id, "label": g.label, "description": g.description,
            "mode": tools::mode_of(&modes, g.id), "default_mode": g.default_mode, "tools": tools_in
        }));
    }
    for p in tools::enabled_packs(app, id).await? {
        let gid = format!("pack:{}", p.id);
        out.push(json!({
            "id": gid, "label": p.name, "description": p.description,
            "mode": tools::mode_of(&modes, &gid), "default_mode": "allow",
            "tools": p.tools.iter().map(|t| json!({ "name": t.name, "title": t.title, "risk": t.risk })).collect::<Vec<_>>()
        }));
    }
    let changed = appmcp::changed(app, id).await?;
    for e in appmcp::load(app, id).await?.servers {
        let gid = appmcp::group_id(&e.server.id);
        let mode = tools::mode_of(&modes, &gid);
        out.push(appmcp::group_json(&e, &mode, changed.contains(&gid)));
    }
    Ok(out)
}

/// `POST /api/instances/:id/app-servers/refresh`: ask the Mac which apps have MCP
/// servers (starting none of them) and list the tools of the ones that are on.
pub async fn app_servers_refresh(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<Value>>> {
    let id = owned_instance(&app, &s, &id).await?;
    refresh_app_servers(&app, &id, None).await?;
    Ok(Json(groups(&app, &id).await?))
}

async fn refresh_app_servers(app: &App, id: &str, only: Option<&str>) -> ApiResult<()> {
    match appmcp::refresh(app, id, only).await {
        Ok(_) => Ok(()),
        Err(e @ (appmcp::RefreshError::Offline | appmcp::RefreshError::Unsupported)) => {
            Err(ApiError::unavailable(e.to_string()))
        }
        Err(appmcp::RefreshError::Other(e)) => {
            tracing::warn!(instance = %id, error = %format!("{e:#}"), "app servers");
            Err(ApiError::unavailable(
                "Couldn\u{2019}t ask the Mac for its app servers. Try again in a moment.",
            ))
        }
    }
}

#[derive(Deserialize)]
pub struct ModeBody {
    mode: String,
}

/// `PUT /api/instances/:id/permissions/:group`.
pub async fn set_permission(
    State(app): State<App>,
    s: Session,
    Path((id, group)): Path<(String, String)>,
    Json(b): Json<ModeBody>,
) -> ApiResult<Json<Value>> {
    let id = owned_instance(&app, &s, &id).await?;
    if !matches!(b.mode.as_str(), "allow" | "ask" | "deny") {
        return Err(ApiError::invalid("mode", "Choose Allow, Ask or Never."));
    }
    let known = catalog::group(&group).is_some()
        || appmcp::is_group(&group)
        || group.strip_prefix("pack:").is_some_and(|p| {
            p.len() <= 24
                && p.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        });
    if !known {
        return Err(ApiError::not_found("permission group"));
    }
    // Turning an app server on approves it as it is now: look at it on the Mac first (and
    // list its tools, so the owner sees them), then remember its fingerprint. If the Mac
    // is off, the last thing it reported is what's approved.
    let mut fingerprint = None;
    if let Some(server) = group.strip_prefix(appmcp::GROUP_PREFIX)
        && b.mode != "deny"
    {
        let _ = refresh_app_servers(&app, &id, Some(server)).await;
        let cache = appmcp::load(&app, &id).await?;
        let entry = cache
            .servers
            .iter()
            .find(|e| e.server.id == server)
            .ok_or_else(|| ApiError::not_found("app server"))?;
        fingerprint = Some(entry.server.fingerprint.clone());
    }
    let (iid, g, m) = (id.clone(), group.clone(), b.mode.clone());
    app.db
        .call(move |c| {
            c.execute(
                "INSERT INTO permissions (instance_id, grp, mode, fingerprint) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(instance_id, grp) DO UPDATE SET mode = excluded.mode,
                   fingerprint = coalesce(excluded.fingerprint, permissions.fingerprint)",
                rusqlite::params![iid, g, m, fingerprint],
            )
        })
        .await?;
    groups(&app, &id)
        .await?
        .into_iter()
        .find(|v| v["id"] == group)
        .map(Json)
        .ok_or_else(|| ApiError::not_found("permission group"))
}

/// `GET /api/instances/:id/approvals`.
pub async fn approvals(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<approvals::ApprovalView>>> {
    let id = owned_instance(&app, &s, &id).await?;
    Ok(Json(approvals::list(&app, &id).await?))
}

#[derive(Deserialize)]
pub struct DecisionBody {
    decision: String,
}

/// `POST /api/instances/:id/approvals/:aid`.
pub async fn decide(
    State(app): State<App>,
    s: Session,
    Path((id, aid)): Path<(String, String)>,
    Json(b): Json<DecisionBody>,
) -> ApiResult<Json<approvals::ApprovalView>> {
    let id = owned_instance(&app, &s, &id).await?;
    let approve = match b.decision.as_str() {
        "approve" => true,
        "deny" => false,
        _ => return Err(ApiError::invalid("decision", "Choose approve or deny.")),
    };
    let v = approvals::decide(&app, &id, &aid, approve).await?;
    publish_instance(&app, &id).await;
    Ok(Json(v))
}

#[derive(Deserialize)]
pub struct ActivityQuery {
    before: Option<String>,
    limit: Option<u32>,
    outcome: Option<String>,
}

/// `GET /api/instances/:id/activity`.
pub async fn activity(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
    Query(q): Query<ActivityQuery>,
) -> ApiResult<Json<Value>> {
    let id = owned_instance(&app, &s, &id).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let before: i64 = q.before.and_then(|b| b.parse().ok()).unwrap_or(i64::MAX);
    let outcome = q
        .outcome
        .filter(|o| matches!(o.as_str(), "ok" | "error" | "denied" | "pending"));
    let items: Vec<Value> = app
        .db
        .call(move |c| {
            let mut st = c.prepare(
                "SELECT id, at, tool, outcome, duration_ms, detail, error_code, client FROM activity
                 WHERE instance_id = ?1 AND id < ?2 AND (?3 IS NULL OR outcome = ?3) ORDER BY id DESC LIMIT ?4",
            )?;
            let rows = st.query_map(rusqlite::params![id, before, outcome, limit + 1], |r| {
                let tool: String = r.get(2)?;
                Ok(json!({
                    "id": r.get::<_, i64>(0)?.to_string(), "at": rfc3339(r.get(1)?), "title": tools::title_of(&tool), "tool": tool,
                    "outcome": r.get::<_, String>(3)?, "duration_ms": r.get::<_, i64>(4)?, "detail": r.get::<_, Option<String>>(5)?,
                    "error_code": r.get::<_, Option<String>>(6)?, "client": r.get::<_, Option<String>>(7)?
                }))
            })?;
            rows.collect()
        })
        .await?;
    let mut items = items;
    let next = if items.len() > limit as usize {
        items.truncate(limit as usize);
        items
            .last()
            .and_then(|v| v["id"].as_str().map(str::to_owned))
    } else {
        None
    };
    Ok(Json(json!({ "items": items, "next": next })))
}

/// `GET /api/instances/:id/snapshots`.
pub async fn snapshots(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<crate::snapshots::SnapshotView>>> {
    let id = owned_instance(&app, &s, &id).await?;
    Ok(Json(crate::snapshots::list(&app, &id).await?))
}

#[derive(Deserialize)]
pub struct SnapshotBody {
    name: String,
}

/// `POST /api/instances/:id/snapshots`.
pub async fn snapshot_create(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
    Json(b): Json<SnapshotBody>,
) -> ApiResult<Json<crate::snapshots::SnapshotView>> {
    let id = owned_instance(&app, &s, &id).await?;
    use crate::snapshots::CreateError;
    crate::snapshots::create(&app, &id, &b.name, "owner")
        .await
        .map(Json)
        .map_err(|e| match e {
            CreateError::Invalid(m) => ApiError::invalid("name", m),
            CreateError::Limit(m) => ApiError::budget(m),
            CreateError::Failed(m) => ApiError::from_machine(m),
        })
}

/// `POST /api/instances/:id/snapshots/:sid/restore`.
pub async fn snapshot_restore(
    State(app): State<App>,
    s: Session,
    Path((id, sid)): Path<(String, String)>,
) -> ApiResult<Json<InstanceView>> {
    let id = owned_instance(&app, &s, &id).await?;
    crate::snapshots::restore(&app, &id, &sid)
        .await
        .map_err(ApiError::from_machine)?;
    publish_instance(&app, &id).await;
    Ok(Json(instance_view(&app, &id).await?))
}

/// `DELETE /api/instances/:id/snapshots/:sid`.
pub async fn snapshot_delete(
    State(app): State<App>,
    s: Session,
    Path((id, sid)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let id = owned_instance(&app, &s, &id).await?;
    crate::snapshots::delete(&app, &id, &sid)
        .await
        .map_err(ApiError::from_machine)?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub struct KeepBody {
    kept: bool,
}

/// `PUT /api/instances/:id/snapshots/:sid`: mark to keep (or not).
pub async fn snapshot_keep(
    State(app): State<App>,
    s: Session,
    Path((id, sid)): Path<(String, String)>,
    Json(b): Json<KeepBody>,
) -> ApiResult<Json<crate::snapshots::SnapshotView>> {
    let id = owned_instance(&app, &s, &id).await?;
    crate::snapshots::set_kept(&app, &id, &sid, b.kept)
        .await
        .map(Json)
        .map_err(|_| ApiError::not_found("Snapshot"))
}

/// `GET /api/instances/:id/snapshot-usage`.
pub async fn snapshot_usage(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
) -> ApiResult<Json<crate::snapshots::Usage>> {
    let id = owned_instance(&app, &s, &id).await?;
    Ok(Json(crate::snapshots::usage(&app, &id).await))
}

/// `GET /api/instances/:id/connections`.
pub async fn connections(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let id = owned_instance(&app, &s, &id).await?;
    let (grants, tokens): (Vec<Value>, Vec<Value>) = app
        .db
        .call(move |c| {
            let mut st = c.prepare(
                "SELECT id, name, client_id, created_at, last_used_at FROM grants
                 WHERE instance_id = ?1 AND revoked = 0 AND client_id IS NOT NULL ORDER BY created_at DESC",
            )?;
            let grants = st
                .query_map([&id], |r| {
                    Ok(json!({ "id": r.get::<_, String>(0)?, "client_name": r.get::<_, String>(1)?, "client_id": r.get::<_, String>(2)?,
                               "created_at": rfc3339(r.get(3)?), "last_used_at": rfc3339_opt(r.get(4)?) }))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut st = c.prepare(
                "SELECT g.id, g.name, t.prefix, g.created_at, t.expires_at, g.last_used_at FROM grants g JOIN tokens t ON t.grant_id = g.id
                 WHERE g.instance_id = ?1 AND g.revoked = 0 AND g.client_id IS NULL AND t.kind = 'personal' ORDER BY g.created_at DESC",
            )?;
            let tokens = st
                .query_map([&id], |r| {
                    Ok(json!({ "id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?, "prefix": r.get::<_, String>(2)?,
                               "created_at": rfc3339(r.get(3)?), "expires_at": rfc3339_opt(r.get(4)?), "last_used_at": rfc3339_opt(r.get(5)?) }))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok((grants, tokens))
        })
        .await?;
    Ok(Json(
        json!({ "mcp_url": app.config.mcp_url(), "grants": grants, "tokens": tokens }),
    ))
}

#[derive(Deserialize)]
pub struct TokenBody {
    name: String,
    expires_in_days: Option<i64>,
}

/// `POST /api/instances/:id/tokens`: the secret is returned once.
pub async fn token_create(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
    Json(b): Json<TokenBody>,
) -> ApiResult<Json<Value>> {
    s.require_confirmed()?;
    let id = owned_instance(&app, &s, &id).await?;
    let name = crate::util::clean_name(&b.name, 60)
        .ok_or_else(|| ApiError::invalid("name", "Name the token so you know where it's used."))?;
    if let Some(d) = b.expires_in_days
        && !(1..=3650).contains(&d)
    {
        return Err(ApiError::invalid(
            "expires_in_days",
            "Choose 1 to 3650 days, or no expiry.",
        ));
    }
    let secret = crypto::token("dot_pat_");
    let hash = crypto::sha256(secret.as_bytes());
    let gid = crypto::random_id::<10>();
    let t = now();
    let expires = b.expires_in_days.map(|d| t + d * 86_400);
    let prefix = secret.chars().take(12).collect::<String>();
    let (g2, uid, iid, n2, p2) = (
        gid.clone(),
        s.user.id.clone(),
        id.clone(),
        name.clone(),
        prefix.clone(),
    );
    app.db
        .call(move |c| {
            let tx = c.transaction()?;
            tx.execute(
                "INSERT INTO grants (id, user_id, instance_id, client_id, name, created_at) VALUES (?1, ?2, ?3, NULL, ?4, ?5)",
                rusqlite::params![g2, uid, iid, n2, t],
            )?;
            tx.execute(
                "INSERT INTO tokens (hash, grant_id, kind, prefix, expires_at, created_at) VALUES (?1, ?2, 'personal', ?3, ?4, ?5)",
                rusqlite::params![hash, g2, p2, expires, t],
            )?;
            tx.commit()
        })
        .await?;
    auth::security_event(
        &app,
        &s.user.id,
        "token_created",
        Some(s.ip),
        s.user_agent.clone(),
        Some(name.clone()),
    )
    .await;
    Ok(Json(json!({
        "token": { "id": gid, "name": name, "prefix": prefix, "created_at": rfc3339(t), "expires_at": rfc3339_opt(expires), "last_used_at": null },
        "secret": secret
    })))
}

async fn revoke(
    app: &App,
    s: &Session,
    instance: &str,
    grant: &str,
    personal: bool,
    kind: &'static str,
) -> ApiResult<Json<Value>> {
    let id = owned_instance(app, s, instance).await?;
    let gid = grant.to_owned();
    let name: Option<String> = app
        .db
        .call(move |c| {
            let name: Option<String> = c
                .query_row(
                    "SELECT name FROM grants WHERE id = ?1 AND instance_id = ?2 AND (client_id IS NULL) = ?3 AND revoked = 0",
                    rusqlite::params![gid, id, personal],
                    |r| r.get(0),
                )
                .optional()?;
            if name.is_some() {
                c.execute("UPDATE grants SET revoked = 1 WHERE id = ?1", [&gid])?;
                c.execute("DELETE FROM tokens WHERE grant_id = ?1", [&gid])?;
            }
            Ok(name)
        })
        .await?;
    let name =
        name.ok_or_else(|| ApiError::not_found(if personal { "token" } else { "connection" }))?;
    auth::security_event(
        app,
        &s.user.id,
        kind,
        Some(s.ip),
        s.user_agent.clone(),
        Some(name),
    )
    .await;
    Ok(Json(json!({})))
}

/// `DELETE /api/instances/:id/tokens/:tid`.
pub async fn token_revoke(
    State(app): State<App>,
    s: Session,
    Path((id, tid)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    revoke(&app, &s, &id, &tid, true, "token_revoked").await
}

/// `DELETE /api/instances/:id/grants/:gid`.
pub async fn grant_revoke(
    State(app): State<App>,
    s: Session,
    Path((id, gid)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    revoke(&app, &s, &id, &gid, false, "grant_revoked").await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_hosts_must_be_private() {
        assert!(check_remote("100.101.1.2:7466").is_ok(), "Tailscale CGNAT");
        assert!(check_remote("192.168.1.20:7466").is_ok());
        assert!(check_remote("[fd7a:115c:a1e0::1]:7466").is_ok());
        assert!(check_remote("8.8.8.8:7466").is_err());
        assert!(check_remote("mac.example.com:7466").is_err());
        assert!(
            check_resources(2, 8192, 128).is_ok(),
            "any host runs 2 cores"
        );
        assert!(check_resources(1, 8192, 128).is_err());
        assert!(check_resources(2, 1024, 128).is_err());
    }
}
