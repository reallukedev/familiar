//! Update settings and actions (admins), the admin event stream, and per-Mac agent updates.
use super::{instance_view, owned_instance, publish_instance};
use crate::{
    auth::Session,
    error::{ApiError, ApiResult},
    events::Audience,
    state::App,
    updates,
};
use axum::{
    Json,
    extract::{Path, State},
    response::{IntoResponse, Response, sse},
};
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

/// `GET /api/admin/updates`.
pub async fn get(State(app): State<App>, s: Session) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    Ok(Json(updates::status(&app).await))
}

#[derive(Deserialize)]
pub struct SettingsBody {
    channel: Option<String>,
    auto_install: Option<bool>,
    window_start_hour: Option<u8>,
    guest_default_channel: Option<String>,
    guest_auto_install: Option<bool>,
}

fn channel_ok(c: &str) -> bool {
    matches!(c, "stable" | "nightly" | "off")
}

/// `PUT /api/admin/updates`.
pub async fn put(
    State(app): State<App>,
    s: Session,
    Json(b): Json<SettingsBody>,
) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    s.require_confirmed()?;
    let mut settings = updates::settings(&app).await;
    if let Some(c) = b.channel {
        if !channel_ok(&c) {
            return Err(ApiError::invalid(
                "channel",
                "Choose Stable, Nightly or Off.",
            ));
        }
        settings.channel = c;
    }
    if let Some(c) = b.guest_default_channel {
        if !channel_ok(&c) {
            return Err(ApiError::invalid(
                "guest_default_channel",
                "Choose Stable, Nightly or Off.",
            ));
        }
        settings.guest_default_channel = c;
    }
    if let Some(h) = b.window_start_hour {
        if h > 23 {
            return Err(ApiError::invalid(
                "window_start_hour",
                "Choose an hour from 0 to 23.",
            ));
        }
        settings.window_start_hour = h;
    }
    if let Some(v) = b.auto_install {
        settings.auto_install = v;
    }
    if let Some(v) = b.guest_auto_install {
        settings.guest_auto_install = v;
    }
    updates::save_settings(&app, &settings).await?;
    // A channel change should show what that channel offers right away.
    let app2 = app.clone();
    tokio::spawn(async move { updates::check(&app2).await });
    updates::publish(&app).await;
    Ok(Json(updates::status(&app).await))
}

/// `POST /api/admin/updates/check`.
pub async fn check(State(app): State<App>, s: Session) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    if let Some(r) = updates::disabled_reason() {
        return Err(ApiError::unavailable(r));
    }
    app.limiter
        .hit(
            "update_check",
            &s.user.id,
            crate::auth::ratelimit::Rule {
                max: 10,
                window: Duration::from_secs(600),
            },
        )
        .map_err(ApiError::rate_limited)?;
    updates::check(&app).await;
    Ok(Json(updates::status(&app).await))
}

/// `POST /api/admin/updates/install`.
pub async fn install(State(app): State<App>, s: Session) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    s.require_confirmed()?;
    let settings = updates::settings(&app).await;
    let manifest = app.updates.manifest(&settings.channel).ok_or_else(|| {
        ApiError::conflict("There's no update to install. Check for updates first.")
    })?;
    if manifest.version == updates::build::version() {
        return Err(ApiError::conflict("Familiar is already on this version."));
    }
    let app2 = app.clone();
    tokio::spawn(async move {
        let _ = updates::install_server(&app2, &manifest).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    Ok(Json(updates::status(&app).await))
}

/// `POST /api/admin/updates/rollback`.
pub async fn rollback(State(app): State<App>, s: Session) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    s.require_confirmed()?;
    updates::rollback(&app)
        .await
        .map_err(|e| ApiError::conflict(format!("Couldn't roll back: {e:#}.")))?;
    Ok(Json(updates::status(&app).await))
}

/// `GET /api/admin/events`: `updates` and `image` for admins.
pub async fn events(State(app): State<App>, s: Session) -> ApiResult<Response> {
    s.require_admin()?;
    let mut rx = app.events.subscribe();
    let first = updates::status(&app).await;
    let (app2, key) = (app.clone(), s.id_hash.clone());
    let mut recheck = tokio::time::interval(Duration::from_secs(30));
    recheck.tick().await;
    let stream = async_stream::stream! {
        yield Ok::<_, std::convert::Infallible>(sse::Event::default().event("updates").json_data(first).unwrap_or_default());
        loop {
            let next = tokio::select! {
                ev = rx.recv() => ev,
                _ = recheck.tick() => {
                    if crate::auth::still_valid(&app2, &key).await { continue; }
                    break;
                }
            };
            match next {
                Ok(ev) if ev.audience == Audience::Admins => {
                    yield Ok(sse::Event::default().event(ev.kind).json_data(ev.data).unwrap_or_default());
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
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

/// `POST /api/instances/:id/agent/update`.
pub async fn agent_update(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
) -> ApiResult<Json<super::InstanceView>> {
    let id = owned_instance(&app, &s, &id).await?;
    if let Some(r) = updates::disabled_reason() {
        return Err(ApiError::unavailable(r));
    }
    let iid = id.clone();
    let channel: String = app
        .db
        .call(move |c| {
            c.query_row(
                "SELECT agent_channel FROM instances WHERE id = ?1",
                [iid],
                |r| r.get(0),
            )
        })
        .await?;
    let settings = updates::settings(&app).await;
    let ch = updates::effective_channel(&channel, &settings);
    if ch == "off" {
        return Err(ApiError::conflict(
            "Agent updates are off for this Mac. Choose a channel first.",
        ));
    }
    if app.updates.manifest(&ch).is_none() {
        updates::check(&app).await;
    }
    let manifest = app.updates.manifest(&ch).ok_or_else(|| {
        ApiError::unavailable("Couldn't find an agent update. Try again in a few minutes.")
    })?;
    if app.fleet.machine(&id).link().is_none() {
        return Err(ApiError::unavailable(
            "Start the Mac, then update its agent.",
        ));
    }
    let (app2, id2) = (app.clone(), id.clone());
    tokio::spawn(async move {
        if let Err(e) = updates::update_agent(&app2, &id2, &manifest).await {
            tracing::warn!(instance = %id2, error = %format!("{e:#}"), "agent update failed");
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    publish_instance(&app, &id).await;
    Ok(Json(instance_view(&app, &id).await?))
}
