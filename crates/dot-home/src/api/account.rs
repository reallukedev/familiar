//! The signed-in person's own account: name, password, two-factor, sessions, log.
use crate::{
    auth::{self, CONFIRM_SECONDS, Session, USER_COLUMNS, UserView, password, ratelimit, totp},
    db::{now, rfc3339},
    error::{ApiError, ApiResult},
    keys::{self, DataKey, RecoveryKey, Stored},
    state::App,
};
use axum::{
    Json,
    extract::{Path, State},
};
use serde::Deserialize;
use serde_json::{Value, json};
use zeroize::Zeroizing;

async fn user(app: &App, id: &str) -> ApiResult<UserView> {
    let id = id.to_owned();
    Ok(app
        .db
        .call(move |c| {
            c.query_row(
                &format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?1"),
                [id],
                UserView::from_row,
            )
        })
        .await?)
}

/// `GET /api/account`.
pub async fn get(State(app): State<App>, s: Session) -> ApiResult<Json<Value>> {
    let uid = s.user.id.clone();
    let current = s.public_id.clone();
    let sessions = app
        .db
        .call(move |c| {
            let mut st = c.prepare(
                "SELECT public_id, user_agent, ip, created_at, last_seen_at FROM sessions WHERE user_id = ?1 ORDER BY last_seen_at DESC",
            )?;
            let rows = st.query_map([uid], |r| {
                let id: String = r.get(0)?;
                Ok(json!({
                    "id": id, "current": id == current,
                    "user_agent": r.get::<_, Option<String>>(1)?, "ip": r.get::<_, Option<String>>(2)?,
                    "created_at": rfc3339(r.get(3)?), "last_seen_at": rfc3339(r.get(4)?)
                }))
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .await?;
    Ok(Json(json!({ "user": s.user, "sessions": sessions })))
}

#[derive(Deserialize)]
pub struct PatchBody {
    display_name: String,
}

/// `PATCH /api/account`.
pub async fn patch(
    State(app): State<App>,
    s: Session,
    Json(b): Json<PatchBody>,
) -> ApiResult<Json<UserView>> {
    let name = crate::util::clean_name(&b.display_name, 60)
        .ok_or_else(|| ApiError::invalid("display_name", "Enter a name of up to 60 characters."))?;
    let uid = s.user.id.clone();
    app.db
        .call(move |c| {
            c.execute(
                "UPDATE users SET display_name = ?2 WHERE id = ?1",
                [uid, name],
            )
        })
        .await?;
    Ok(Json(user(&app, &s.user.id).await?))
}

#[derive(Deserialize)]
pub struct ConfirmBody {
    password: String,
    #[serde(default)]
    code: Option<String>,
}

/// `POST /api/account/confirm`: unlock sensitive actions for 10 minutes, and unlock the
/// person's encrypted Macs if a restart locked them.
pub async fn confirm(
    State(app): State<App>,
    s: Session,
    Json(b): Json<ConfirmBody>,
) -> ApiResult<Json<Value>> {
    app.limiter
        .hit("confirm", &s.user.id, ratelimit::TOTP)
        .map_err(ApiError::rate_limited)?;
    let uid = s.user.id.clone();
    let (hash, sealed, last_step): (String, Option<Vec<u8>>, i64) = app
        .db
        .call(move |c| {
            c.query_row(
                "SELECT password_hash, totp_secret, totp_last_step FROM users WHERE id = ?1",
                [uid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
        })
        .await?;
    let plain = Zeroizing::new(b.password.clone());
    if !password::verify(b.password, Some(hash)).await {
        return Err(ApiError::invalid("password", "That password isn't right."));
    }
    if let Some(sealed) = sealed {
        let secret = app.key.open(&format!("totp:{}", s.user.id), &sealed)?;
        let step = b
            .code
            .as_deref()
            .and_then(|c| totp::verify(&secret, c, now(), last_step))
            .ok_or_else(|| {
                ApiError::invalid(
                    "code",
                    "Enter the current code from your authenticator app.",
                )
            })?;
        let uid = s.user.id.clone();
        let claimed = app
            .db
            .call(move |c| {
                c.execute(
                    "UPDATE users SET totp_last_step = ?2 WHERE id = ?1 AND totp_last_step < ?2",
                    rusqlite::params![uid, step],
                )
            })
            .await?;
        if claimed != 1 {
            return Err(ApiError::invalid(
                "code",
                "That code was already used. Wait for the next one.",
            ));
        }
    }
    let until = now() + CONFIRM_SECONDS;
    let hash = s.id_hash.clone();
    app.db
        .call(move |c| {
            c.execute(
                "UPDATE sessions SET confirmed_until = ?2 WHERE id_hash = ?1",
                rusqlite::params![hash, until],
            )
        })
        .await?;
    // Password and code are both proven: this is a sign-in as far as the key goes.
    let recovery = if app.fleet.keys().is_unlocked(&s.user.id) {
        None
    } else {
        match super::session::open_keys(&app, &s.user.id, &plain).await {
            Some(o) => {
                super::session::unlock(&app, &s.user.id, o, Some(s.ip), s.user_agent.clone()).await
            }
            None => None,
        }
    };
    let mut body = json!({ "until": rfc3339(until) });
    if let Some(key) = recovery {
        body["recovery_key"] = json!(key);
    }
    Ok(Json(body))
}

#[derive(Deserialize)]
pub struct PasswordBody {
    current: String,
    password: String,
}

/// `POST /api/account/password`: signs out every other session.
pub async fn password(
    State(app): State<App>,
    s: Session,
    Json(b): Json<PasswordBody>,
) -> ApiResult<Json<Value>> {
    app.limiter
        .hit("confirm", &s.user.id, ratelimit::TOTP)
        .map_err(ApiError::rate_limited)?;
    let uid = s.user.id.clone();
    let hash: String = app
        .db
        .call(move |c| {
            c.query_row(
                "SELECT password_hash FROM users WHERE id = ?1",
                [uid],
                |r| r.get(0),
            )
        })
        .await?;
    let current = Zeroizing::new(b.current.clone());
    if !password::verify(b.current, Some(hash)).await {
        return Err(ApiError::invalid(
            "current",
            "Your current password isn't right.",
        ));
    }
    password::check_policy(&b.password, &s.user.username)
        .map_err(|m| ApiError::invalid("password", m))?;
    // The data key stays the same; only its password wrap changes.
    let (key, stored, recovery_key) =
        rewrap_for_new_password(&app, &s.user.id, &current, &b.password).await?;
    let new_hash = password::hash(b.password).await?;
    let (uid, keep) = (s.user.id.clone(), s.id_hash.clone());
    app.db
        .call(move |c| {
            let tx = c.transaction()?;
            tx.execute(
                "UPDATE users SET password_hash = ?2 WHERE id = ?1",
                rusqlite::params![uid, new_hash],
            )?;
            tx.execute(
                "DELETE FROM sessions WHERE user_id = ?1 AND id_hash != ?2",
                rusqlite::params![uid, keep],
            )?;
            stored.save(&tx, &uid)?;
            tx.commit()
        })
        .await?;
    app.fleet.keys().insert(&s.user.id, key);
    auth::security_event(
        &app,
        &s.user.id,
        "password_changed",
        Some(s.ip),
        s.user_agent.clone(),
        None,
    )
    .await;
    let mut body = json!({});
    if let Some(shown) = recovery_key {
        auth::security_event(
            &app,
            &s.user.id,
            "recovery_key_created",
            Some(s.ip),
            s.user_agent,
            None,
        )
        .await;
        body["recovery_key"] = json!(shown);
    }
    Ok(Json(body))
}

/// The data key wrapped with `new` (the current password, or the unlocked key, opens
/// it). Accounts without keys yet get new ones, with a recovery key to show.
async fn rewrap_for_new_password(
    app: &App,
    user_id: &str,
    current: &str,
    new: &str,
) -> ApiResult<(DataKey, Stored, Option<String>)> {
    let (uid, current, new) = (
        user_id.to_owned(),
        Zeroizing::new(current.to_owned()),
        Zeroizing::new(new.to_owned()),
    );
    let unlocked = app.fleet.keys().get(user_id);
    match keys::load(&app.db, user_id).await? {
        Some(stored) => tokio::task::spawn_blocking(move || {
            let key = unlocked
                .or_else(|| stored.open_with_password(&uid, &current))
                .ok_or_else(|| {
                    anyhow::anyhow!("neither the current password nor memory holds the data key")
                })?;
            let next = stored.with_password(&uid, &new, &key)?;
            anyhow::Ok((key, next, None))
        })
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::from),
        None => tokio::task::spawn_blocking(move || {
            let key = DataKey::random();
            let recovery = RecoveryKey::random();
            let stored = Stored::seal(&uid, &new, &key, &recovery)?;
            anyhow::Ok((key, stored, Some(recovery.display())))
        })
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::from),
    }
}

/// `POST /api/account/recovery-key`: a new recovery key, shown once; the old one stops
/// working. Needs a recent password confirmation (which also unlocks).
pub async fn recovery_key(State(app): State<App>, s: Session) -> ApiResult<Json<Value>> {
    s.require_confirmed()?;
    let key = app
        .fleet
        .keys()
        .get(&s.user.id)
        .ok_or_else(ApiError::locked)?;
    let shown = keys::new_recovery_key(&app.db, &s.user.id, &key).await?;
    auth::security_event(
        &app,
        &s.user.id,
        "recovery_key_created",
        Some(s.ip),
        s.user_agent,
        None,
    )
    .await;
    Ok(Json(json!({ "recovery_key": shown })))
}

/// `POST /api/account/recovery-key/saved`: the person saved their recovery key.
pub async fn recovery_key_saved(State(app): State<App>, s: Session) -> ApiResult<Json<Value>> {
    keys::mark_saved(&app.db, &s.user.id).await?;
    Ok(Json(json!({})))
}

/// `POST /api/account/totp`: start enrollment (not active until confirmed).
/// With two-factor already on this changes the code: the current secret keeps
/// working until the new one is confirmed, so there is never a moment without
/// two-factor. Changing it asks for the password again.
pub async fn totp_begin(State(app): State<App>, s: Session) -> ApiResult<Json<Value>> {
    if s.user.totp_enabled {
        s.require_confirmed()?;
    }
    let secret = totp::new_secret();
    let sealed = app.key.seal(&format!("totp:{}", s.user.id), &secret);
    let uid = s.user.id.clone();
    app.db
        .call(move |c| {
            c.execute(
                "UPDATE users SET totp_pending = ?2 WHERE id = ?1",
                rusqlite::params![uid, sealed],
            )
        })
        .await?;
    let host = app
        .config
        .public_url
        .host_str()
        .unwrap_or("Familiar")
        .to_owned();
    Ok(Json(json!({
        "secret": totp::encode(&secret),
        "otpauth_url": totp::otpauth_url(&secret, &format!("Familiar ({host})"), &s.user.username),
    })))
}

#[derive(Deserialize)]
pub struct CodeBody {
    code: String,
}

/// `POST /api/account/totp/confirm`.
pub async fn totp_confirm(
    State(app): State<App>,
    s: Session,
    Json(b): Json<CodeBody>,
) -> ApiResult<Json<UserView>> {
    app.limiter
        .hit("totp", &s.user.id, ratelimit::TOTP)
        .map_err(ApiError::rate_limited)?;
    let uid = s.user.id.clone();
    let pending: Option<Vec<u8>> = app
        .db
        .call(move |c| {
            c.query_row("SELECT totp_pending FROM users WHERE id = ?1", [uid], |r| {
                r.get(0)
            })
        })
        .await?;
    let sealed = pending
        .ok_or_else(|| ApiError::conflict("Start setting up two-factor authentication again."))?;
    let secret = app.key.open(&format!("totp:{}", s.user.id), &sealed)?;
    let step = totp::verify(&secret, &b.code, now(), 0).ok_or_else(|| {
        ApiError::invalid(
            "code",
            "That code isn't right. Check the time on your phone and try the newest code.",
        )
    })?;
    let uid = s.user.id.clone();
    app.db
        .call(move |c| {
            c.execute(
                "UPDATE users SET totp_secret = totp_pending, totp_pending = NULL, totp_last_step = ?2 WHERE id = ?1",
                rusqlite::params![uid, step],
            )
        })
        .await?;
    auth::security_event(
        &app,
        &s.user.id,
        if s.user.totp_enabled {
            "totp_changed"
        } else {
            "totp_enabled"
        },
        Some(s.ip),
        s.user_agent.clone(),
        None,
    )
    .await;
    Ok(Json(user(&app, &s.user.id).await?))
}

/// `DELETE /api/account/sessions/:id`.
pub async fn revoke_session(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let uid = s.user.id.clone();
    let n = app
        .db
        .call(move |c| {
            c.execute(
                "DELETE FROM sessions WHERE public_id = ?1 AND user_id = ?2",
                [id, uid],
            )
        })
        .await?;
    if n == 0 {
        return Err(ApiError::not_found("session"));
    }
    auth::security_event(
        &app,
        &s.user.id,
        "session_revoked",
        Some(s.ip),
        s.user_agent,
        None,
    )
    .await;
    Ok(Json(json!({})))
}

/// `GET /api/account/security`.
pub async fn security(State(app): State<App>, s: Session) -> ApiResult<Json<Vec<Value>>> {
    let uid = s.user.id.clone();
    Ok(Json(
        app.db
            .call(move |c| {
                let mut st = c.prepare(
                    "SELECT id, at, kind, ip, user_agent, detail FROM security_events WHERE user_id = ?1 ORDER BY id DESC LIMIT 100",
                )?;
                let rows = st.query_map([uid], |r| {
                    Ok(json!({
                        "id": r.get::<_, i64>(0)?.to_string(), "at": rfc3339(r.get(1)?), "kind": r.get::<_, String>(2)?,
                        "ip": r.get::<_, Option<String>>(3)?, "user_agent": r.get::<_, Option<String>>(4)?,
                        "detail": r.get::<_, Option<String>>(5)?
                    }))
                })?;
                rows.collect()
            })
            .await?,
    ))
}
