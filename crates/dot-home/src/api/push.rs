//! Web Push subscriptions (docs/home/API.md "Push notifications").
use crate::{
    auth::Session,
    db::now,
    error::{ApiError, ApiResult},
    state::App,
};
use axum::{Json, extract::State};
use serde::Deserialize;
use serde_json::{Value, json};

/// `GET /api/push/key`: the VAPID public key for `pushManager.subscribe`.
pub async fn key(State(app): State<App>, _s: Session) -> Json<Value> {
    Json(json!({ "public_key": app.push.public_key() }))
}

#[derive(Deserialize)]
pub struct Keys {
    p256dh: String,
    auth: String,
}

#[derive(Deserialize)]
pub struct SubscribeBody {
    endpoint: String,
    keys: Keys,
}

/// `POST /api/push/subscriptions`: remember this browser. Subscribing again with the
/// same endpoint updates it (and moves it to whoever is signed in now).
pub async fn subscribe(
    State(app): State<App>,
    s: Session,
    Json(b): Json<SubscribeBody>,
) -> ApiResult<Json<Value>> {
    crate::push::validate(&b.endpoint, &b.keys.p256dh, &b.keys.auth)
        .map_err(|(field, message)| ApiError::invalid(field, message))?;
    let (uid, sid, agent, t) = (
        s.user.id.clone(),
        s.public_id.clone(),
        s.user_agent.clone(),
        now(),
    );
    let max = crate::push::MAX_PER_USER;
    let stored = app
        .db
        .call(move |c| {
            let tx = c.transaction()?;
            tx.execute(
                "INSERT INTO push_subscriptions (endpoint, user_id, p256dh, auth, session_id, user_agent, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(endpoint) DO UPDATE SET user_id = excluded.user_id, p256dh = excluded.p256dh,
                   auth = excluded.auth, session_id = excluded.session_id, user_agent = excluded.user_agent",
                rusqlite::params![b.endpoint, uid, b.keys.p256dh, b.keys.auth, sid, agent, t],
            )?;
            // Past the limit, the oldest browsers make room.
            tx.execute(
                "DELETE FROM push_subscriptions WHERE user_id = ?1 AND endpoint NOT IN
                   (SELECT endpoint FROM push_subscriptions WHERE user_id = ?1
                    ORDER BY created_at DESC, rowid DESC LIMIT ?2)",
                rusqlite::params![uid, max],
            )?;
            tx.commit()
        })
        .await;
    stored?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub struct UnsubscribeBody {
    endpoint: String,
}

/// `DELETE /api/push/subscriptions`: forget this browser. Unknown endpoints are fine.
pub async fn unsubscribe(
    State(app): State<App>,
    s: Session,
    Json(b): Json<UnsubscribeBody>,
) -> ApiResult<Json<Value>> {
    let uid = s.user.id.clone();
    app.db
        .call(move |c| {
            c.execute(
                "DELETE FROM push_subscriptions WHERE endpoint = ?1 AND user_id = ?2",
                [b.endpoint, uid],
            )
        })
        .await?;
    Ok(Json(json!({})))
}
