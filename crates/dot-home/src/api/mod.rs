//! Portal API (docs/home/API.md).
mod account;
mod admin;
mod apps;
mod clipboard;
mod instances;
pub mod preferences;
mod push;
mod review;
mod screen;
mod session;
pub mod storage;
mod updates;

use crate::{
    auth::Session,
    db::{rfc3339, rfc3339_opt},
    error::{ApiError, ApiResult},
    state::App,
};
use axum::{
    Router,
    routing::{delete, get, post, put},
};
use rusqlite::OptionalExtension;
use serde::Serialize;
use serde_json::Value;

/// Every `/api` route.
pub fn router() -> Router<App> {
    Router::new()
        .route("/session", get(session::info))
        .route("/setup", post(session::setup))
        .route("/login", post(session::login))
        .route("/login/totp", post(session::login_totp))
        .route("/logout", post(session::logout))
        .route("/invites/{code}", get(session::invite_info))
        .route("/invites/{code}/accept", post(session::invite_accept))
        .route("/account", get(account::get).patch(account::patch))
        .route("/account/password", post(account::password))
        .route("/account/totp", post(account::totp_begin))
        .route("/account/totp/confirm", post(account::totp_confirm))
        .route("/account/sessions/{id}", delete(account::revoke_session))
        .route("/account/confirm", post(account::confirm))
        .route("/account/recovery-key", post(account::recovery_key))
        .route(
            "/account/recovery-key/saved",
            post(account::recovery_key_saved),
        )
        .route("/account/security", get(account::security))
        .route(
            "/account/preferences",
            get(preferences::get).put(preferences::put),
        )
        .route("/push/key", get(push::key))
        .route(
            "/push/subscriptions",
            post(push::subscribe).delete(push::unsubscribe),
        )
        .route("/instances", get(instances::list).post(instances::create))
        .route(
            "/instances/{id}",
            get(instances::get)
                .patch(instances::patch)
                .delete(instances::remove),
        )
        .route("/instances/{id}/power", post(instances::power))
        .route("/instances/{id}/encrypt", post(instances::encrypt))
        .route(
            "/instances/{id}/storage/discard-memory",
            post(storage::discard_memory),
        )
        .route("/storage", get(storage::mine))
        .route("/instances/{id}/thumbnail", get(instances::thumbnail))
        .route("/instances/{id}/events", get(instances::events))
        .route("/instances/{id}/screen", get(screen::screen))
        .route("/instances/{id}/permissions", get(instances::permissions))
        .route(
            "/instances/{id}/permissions/mode",
            get(review::mode).put(review::set_mode),
        )
        .route(
            "/instances/{id}/permissions/{group}",
            put(instances::set_permission),
        )
        .route(
            "/instances/{id}/app-servers/refresh",
            post(instances::app_servers_refresh),
        )
        .route("/instances/{id}/approvals", get(instances::approvals))
        .route("/instances/{id}/approvals/{aid}", post(instances::decide))
        .route("/instances/{id}/review", get(review::list))
        .route(
            "/instances/{id}/review/accept-all",
            post(review::accept_all),
        )
        .route("/instances/{id}/review/{item}", post(review::decide))
        .route("/instances/{id}/activity", get(instances::activity))
        .route(
            "/instances/{id}/snapshots",
            get(instances::snapshots).post(instances::snapshot_create),
        )
        .route(
            "/instances/{id}/snapshots/{sid}/restore",
            post(instances::snapshot_restore),
        )
        .route(
            "/instances/{id}/snapshots/{sid}",
            put(instances::snapshot_keep).delete(instances::snapshot_delete),
        )
        .route(
            "/instances/{id}/snapshot-usage",
            get(instances::snapshot_usage),
        )
        .route("/instances/{id}/connections", get(instances::connections))
        .route("/instances/{id}/tokens", post(instances::token_create))
        .route(
            "/instances/{id}/tokens/{tid}",
            delete(instances::token_revoke),
        )
        .route(
            "/instances/{id}/grants/{gid}",
            delete(instances::grant_revoke),
        )
        .route(
            "/instances/{id}/clipboard",
            get(clipboard::get).put(clipboard::put),
        )
        .route("/instances/{id}/apps", get(apps::list))
        .route("/instances/{id}/apps/install", post(apps::install))
        .route("/instances/{id}/apps/custom", post(apps::custom_add))
        .route(
            "/instances/{id}/apps/custom/{app}",
            delete(apps::custom_remove),
        )
        .route("/instances/{id}/apps/{app}", put(apps::set_enabled))
        .route(
            "/oauth/requests/{id}",
            get(crate::oauth::request_info).post(crate::oauth::decide),
        )
        .route("/admin/overview", get(admin::overview))
        .route("/admin/users", get(admin::users))
        .route(
            "/admin/users/{id}",
            axum::routing::patch(admin::user_patch).delete(admin::user_delete),
        )
        .route("/admin/users/{id}/reset", post(admin::user_reset))
        .route("/admin/users/{id}/storage", put(storage::set_overrides))
        .route("/admin/storage", get(storage::admin))
        .route("/admin/storage/defaults", put(storage::set_defaults))
        .route("/admin/instances", get(admin::instances))
        .route("/admin/instances/{id}/stop", post(admin::instance_stop))
        .route(
            "/admin/invites",
            get(admin::invites).post(admin::invite_create),
        )
        .route("/admin/invites/{id}", delete(admin::invite_delete))
        .route("/admin/settings", put(admin::settings))
        .route("/admin/images", get(admin::images))
        .route(
            "/admin/images/builder",
            post(admin::builder_start).delete(admin::builder_delete),
        )
        .route("/admin/images/promote", post(admin::builder_promote))
        .route("/admin/updates", get(updates::get).put(updates::put))
        .route("/admin/updates/check", post(updates::check))
        .route("/admin/updates/install", post(updates::install))
        .route("/admin/updates/rollback", post(updates::rollback))
        .route("/admin/events", get(updates::events))
        .route("/instances/{id}/agent/update", post(updates::agent_update))
}

/// The instance id if `session` owns it; otherwise 404 (never revealing others' Macs).
pub async fn owned_instance(app: &App, session: &Session, id: &str) -> ApiResult<String> {
    let (id, uid) = (id.to_owned(), session.user.id.clone());
    app.db
        .call(move |c| {
            c.query_row(
                "SELECT id FROM instances WHERE id = ?1 AND user_id = ?2",
                [id, uid],
                |r| r.get(0),
            )
            .optional()
        })
        .await?
        .ok_or_else(|| ApiError::not_found("Mac"))
}

/// `Instance` in the API.
#[derive(Debug, Serialize)]
pub struct InstanceView {
    id: String,
    name: String,
    driver: String,
    state: crate::vm::State,
    state_detail: Option<String>,
    paused: bool,
    cpus: u32,
    memory_mb: u32,
    disk_gb: u32,
    sleep_after_minutes: i64,
    hibernate_after_minutes: i64,
    snapshot_keep_days: i64,
    snapshot_max_gb: u64,
    remote_host: Option<String>,
    created_at: String,
    last_active_at: Option<String>,
    booted_at: Option<String>,
    guest: Option<Value>,
    mcp_url: String,
    counts: Counts,
    agent: Value,
    /// Disks, snapshots and saved memory are encrypted with the owner's key.
    pub encrypted: bool,
    /// Encrypted, and the owner's key isn't unlocked since the server started.
    locked: bool,
    /// A Mac on this server made before encryption (image builders never are).
    pub can_encrypt: bool,
    /// 0 to 1 while encrypting.
    encrypt_progress: Option<f64>,
}

#[derive(Debug, Serialize)]
struct Counts {
    calls_24h: i64,
    errors_24h: i64,
    pending_approvals: i64,
    pending_review: i64,
}

/// Build the API view of one instance.
pub async fn instance_view(app: &App, id: &str) -> ApiResult<InstanceView> {
    let iid = id.to_owned();
    let t = crate::db::now();
    let row = app
        .db
        .call(move |c| {
            c.query_row(
                "SELECT name, driver, paused, cpus, memory_mb, disk_gb, sleep_after_minutes, hibernate_after_minutes,
                        remote_host, created_at, last_active_at, booted_at, guest_info, agent_channel, agent_version,
                        (SELECT COUNT(*) FROM activity WHERE instance_id = i.id AND at > ?2),
                        (SELECT COUNT(*) FROM activity WHERE instance_id = i.id AND at > ?2 AND outcome = 'error'),
                        (SELECT COUNT(*) FROM approvals WHERE instance_id = i.id AND status = 'pending' AND expires_at > ?3),
                        snapshot_keep_days, snapshot_max_gb,
                        (SELECT COUNT(*) FROM review_items WHERE instance_id = i.id AND status = 'pending'),
                        encrypted, is_builder, user_id
                 FROM instances i WHERE id = ?1",
                rusqlite::params![iid, t - 86_400, t],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, bool>(2)?,
                        r.get::<_, u32>(3)?,
                        r.get::<_, u32>(4)?,
                        r.get::<_, u32>(5)?,
                        r.get::<_, i64>(6)?,
                        r.get::<_, i64>(7)?,
                        r.get::<_, Option<String>>(8)?,
                        r.get::<_, i64>(9)?,
                        r.get::<_, Option<i64>>(10)?,
                        r.get::<_, Option<i64>>(11)?,
                        r.get::<_, Option<String>>(12)?,
                        (r.get::<_, i64>(15)?, r.get::<_, i64>(16)?, r.get::<_, i64>(17)?),
                        r.get::<_, String>(13)?,
                        r.get::<_, Option<String>>(14)?,
                        (r.get::<_, i64>(18)?, r.get::<_, u64>(19)?),
                        r.get::<_, i64>(20)?,
                        (r.get::<_, bool>(21)?, r.get::<_, bool>(22)?, r.get::<_, String>(23)?),
                    ))
                },
            )
            .optional()
        })
        .await?
        .ok_or_else(|| ApiError::not_found("Mac"))?;
    let machine = app.fleet.machine(id);
    let status = machine.status();
    let (encrypted, is_builder, owner) = row.18;
    let encrypted = encrypted && !is_builder;
    let can_encrypt = row.1 == "qemu" && !encrypted && !is_builder;
    let link = machine.link();
    let settings = crate::updates::settings(app).await;
    let effective = crate::updates::effective_channel(&row.14, &settings);
    let version = link
        .as_ref()
        .map(|l| l.agent_version.clone())
        .or(row.15.clone());
    let available = app
        .updates
        .manifest(&effective)
        .map(|m| m.version)
        .filter(|v| version.as_deref() != Some(v.as_str()));
    let agent = serde_json::json!({
        "version": version,
        "channel": row.14,
        "effective_channel": effective,
        "available": available,
        "updating": app.updates.is_updating(id),
        "can_update": link.as_ref().is_some_and(|l| l.info.capabilities.iter().any(|c| c == "update")),
    });
    Ok(InstanceView {
        id: id.to_owned(),
        name: row.0,
        driver: row.1,
        state: status.state,
        state_detail: status.detail,
        paused: row.2,
        cpus: row.3,
        memory_mb: row.4,
        disk_gb: row.5,
        sleep_after_minutes: row.6,
        hibernate_after_minutes: row.7,
        snapshot_keep_days: row.16.0,
        snapshot_max_gb: row.16.1,
        remote_host: row.8,
        created_at: rfc3339(row.9),
        last_active_at: rfc3339_opt(row.10),
        booted_at: rfc3339_opt(row.11),
        guest: row.12.and_then(|g| serde_json::from_str(&g).ok()),
        mcp_url: app.config.mcp_url(),
        counts: Counts {
            calls_24h: row.13.0,
            errors_24h: row.13.1,
            pending_approvals: row.13.2,
            pending_review: row.17,
        },
        agent,
        locked: encrypted && !app.fleet.keys().is_unlocked(&owner),
        can_encrypt,
        encrypt_progress: machine.encrypt_progress(),
        encrypted,
    })
}

/// Publish the instance's current view to its owner.
pub async fn publish_instance(app: &App, id: &str) {
    if let Ok(view) = instance_view(app, id).await {
        app.events.instance(
            id,
            "instance",
            serde_json::to_value(view).unwrap_or_default(),
        );
    }
}
