//! First-run setup, sign-in (with TOTP), sign-out and invites.
use crate::{
    auth::{
        self, ClientIp, MaybeSession, USER_COLUMNS, UserAgent, UserView, password, ratelimit, totp,
    },
    crypto,
    db::now,
    error::{ApiError, ApiResult},
    keys::{self, DataKey, Opened, RecoveryKey, Stored},
    state::App,
};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use zeroize::Zeroizing;

const FAILURES_BEFORE_LOCK: i64 = 5;

async fn session_json(app: &App, user: Option<UserView>, csrf: Option<String>) -> Value {
    let setup_required = app.setup_required.load(Ordering::Relaxed);
    let (locked, recovery_key_needed) = match &user {
        Some(u) => (
            !app.fleet.keys().is_unlocked(&u.id),
            keys::recovery_key_needed(&app.db, &u.id).await,
        ),
        None => (false, false),
    };
    json!({
        "user": user,
        "csrf": csrf,
        "setup_required": setup_required,
        "require_totp": app.require_totp.load(Ordering::Relaxed),
        "version": crate::updates::build::version(),
        "public_url": app.config.origin(),
        "vm_url": app.config.vm_origin(),
        "locked": locked,
        "recovery_key_needed": recovery_key_needed,
    })
}

/// Keep a person's unwrapped key in memory and let their locked Macs carry on.
/// Returns a recovery key made just now, to show once.
pub async fn unlock(
    app: &App,
    user_id: &str,
    opened: Opened,
    ip: Option<std::net::IpAddr>,
    ua: Option<String>,
) -> Option<String> {
    app.fleet.keys().insert(user_id, opened.key);
    let fleet = app.fleet.clone();
    let uid = user_id.to_owned();
    tokio::spawn(async move { fleet.unlocked(&uid).await });
    if opened.recovery_key.is_some() {
        auth::security_event(app, user_id, "recovery_key_created", ip, ua, None).await;
    }
    opened.recovery_key
}

/// Unwrap (or, the first time, create) a person's key with the password they just
/// proved. A failure is logged and leaves them signed in but locked.
pub async fn open_keys(app: &App, user_id: &str, password: &str) -> Option<Opened> {
    match keys::open_or_create(&app.db, user_id, password).await {
        Ok(Some(o)) => Some(o),
        Ok(None) => {
            tracing::warn!(user = %user_id, "the password didn't open this person's data key");
            None
        }
        Err(e) => {
            tracing::error!(user = %user_id, error = %format!("{e:#}"), "couldn't open the data key");
            None
        }
    }
}

/// `GET /api/session`.
pub async fn info(State(app): State<App>, MaybeSession(s): MaybeSession) -> Json<Value> {
    let (user, csrf) = match s {
        Some(s) => (Some(s.user), Some(s.csrf)),
        None => (None, None),
    };
    Json(session_json(&app, user, csrf).await)
}

/// Start a session and answer with `SessionInfo` plus the cookie.
async fn signed_in(
    app: &App,
    user_id: &str,
    ip: std::net::IpAddr,
    ua: Option<String>,
    recovery_key: Option<String>,
) -> ApiResult<Response> {
    let value = auth::create_session(app, user_id, ip, ua.clone()).await?;
    let hash = crypto::sha256(value.as_bytes());
    let uid = user_id.to_owned();
    let ua_event = ua.clone();
    let (user, csrf, known_device) = app
        .db
        .call(move |c| {
            let user = c.query_row(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?1"), [&uid], UserView::from_row)?;
            let csrf: String = c.query_row("SELECT csrf FROM sessions WHERE id_hash = ?1", [&hash], |r| r.get(0))?;
            let known: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM security_events WHERE user_id = ?1 AND kind = 'sign_in' AND user_agent IS ?2)",
                rusqlite::params![uid, ua],
                |r| r.get(0),
            )?;
            c.execute("UPDATE users SET failed_logins = 0, locked_until = 0, last_seen_at = ?2 WHERE id = ?1", rusqlite::params![uid, now()])?;
            Ok((user, csrf, known))
        })
        .await?;
    auth::security_event(
        app,
        user_id,
        "sign_in",
        Some(ip),
        ua_event,
        (!known_device).then(|| "New browser or device".to_owned()),
    )
    .await;
    let mut body = session_json(app, Some(user), Some(csrf)).await;
    if let Some(key) = recovery_key {
        body["recovery_key"] = json!(key);
    }
    let mut response = Json(body).into_response();
    if let Ok(v) = HeaderValue::from_str(&auth::session_cookie(app.config.secure_cookies(), &value))
    {
        response.headers_mut().insert(header::SET_COOKIE, v);
    }
    Ok(response)
}

fn check_username(u: &str) -> ApiResult<String> {
    let u = u.trim().to_ascii_lowercase();
    let ok = (2..=32).contains(&u.len())
        && u.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(u)
    } else {
        Err(ApiError::invalid(
            "username",
            "Use 2 to 32 lowercase letters, digits, dots, dashes or underscores.",
        ))
    }
}

fn check_display(d: &str) -> ApiResult<String> {
    crate::util::clean_name(d, 60)
        .ok_or_else(|| ApiError::invalid("display_name", "Enter a name of up to 60 characters."))
}

#[derive(Deserialize)]
pub struct SetupBody {
    token: String,
    username: String,
    display_name: String,
    password: String,
}

/// `POST /api/setup`: the first admin, with the one-time token from the server log.
pub async fn setup(
    State(app): State<App>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    Extension(UserAgent(ua)): Extension<UserAgent>,
    Json(body): Json<SetupBody>,
) -> ApiResult<Response> {
    app.limiter
        .hit("login_ip", &ratelimit::ip_key(ip), ratelimit::LOGIN_IP)
        .map_err(ApiError::rate_limited)?;
    let expected = app.db.setting("setup_token_hash").await?;
    let given = hex(&crypto::sha256(body.token.trim().as_bytes()));
    let valid = expected.is_some_and(|e| {
        bool::from(subtle::ConstantTimeEq::ct_eq(
            e.as_bytes(),
            given.as_bytes(),
        ))
    });
    if !valid || !app.setup_required.load(Ordering::Relaxed) {
        return Err(ApiError::forbidden(
            "This setup link isn't valid. Use the link printed in the server log, or run `dot-home setup-link`.",
        ));
    }
    let username = check_username(&body.username)?;
    let display = check_display(&body.display_name)?;
    password::check_policy(&body.password, &username)
        .map_err(|m| ApiError::invalid("password", m))?;
    let plain = Zeroizing::new(body.password.clone());
    let hash = password::hash(body.password).await?;
    let id = crypto::random_id::<10>();
    let uid = id.clone();
    let created = app
        .db
        .call(move |c| {
            let tx = c.transaction()?;
            let users: i64 = tx.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
            if users > 0 {
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO users (id, username, display_name, password_hash, role, created_at) VALUES (?1, ?2, ?3, ?4, 'admin', ?5)",
                rusqlite::params![uid, username, display, hash, now()],
            )?;
            tx.execute("DELETE FROM settings WHERE key = 'setup_token_hash'", [])?;
            tx.commit()?;
            Ok(true)
        })
        .await?;
    if !created {
        return Err(ApiError::conflict(
            "Setup is already finished. Sign in instead.",
        ));
    }
    app.setup_required.store(false, Ordering::Relaxed);
    let _ = std::fs::remove_file(app.config.data_dir.join("setup-url.txt"));
    tracing::info!("first admin created");
    let recovery = match open_keys(&app, &id, &plain).await {
        Some(o) => unlock(&app, &id, o, Some(ip), ua.clone()).await,
        None => None,
    };
    signed_in(&app, &id, ip, ua, recovery).await
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Deserialize)]
pub struct LoginBody {
    username: String,
    password: String,
}

/// `POST /api/login`.
pub async fn login(
    State(app): State<App>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    Extension(UserAgent(ua)): Extension<UserAgent>,
    Json(body): Json<LoginBody>,
) -> ApiResult<Response> {
    app.limiter
        .hit("login_ip", &ratelimit::ip_key(ip), ratelimit::LOGIN_IP)
        .map_err(ApiError::rate_limited)?;
    let username = body.username.trim().to_ascii_lowercase();
    let t = now();
    // Count the attempt before checking the password, atomically, so parallel guesses
    // can't all slip in under the lockout threshold. Success resets the counter.
    let row: Option<(String, String, bool, i64, i64, bool)> = app
        .db
        .call(move |c| {
            c.query_row(
                "UPDATE users SET failed_logins = failed_logins + 1 WHERE username = ?1
                 RETURNING id, password_hash, disabled, failed_logins, locked_until, totp_secret IS NOT NULL",
                [username],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .optional()
        })
        .await?;
    if let Some((_, _, _, _, locked_until, _)) = &row
        && *locked_until > t
    {
        return Err(ApiError::rate_limited(
            u64::try_from(*locked_until - t).unwrap_or(60),
        ));
    }
    let plain = Zeroizing::new(body.password.clone());
    let ok = password::verify(body.password, row.as_ref().map(|r| r.1.clone())).await;
    let Some((id, _, disabled, failures, _, has_totp)) = row else {
        return Err(ApiError::forbidden(
            "That username and password don't match.",
        ));
    };
    if !ok {
        if let Some(wait) = lock_if_needed(&app, &id, failures, t).await? {
            auth::security_event(&app, &id, "locked_out", Some(ip), ua, None).await;
            return Err(ApiError::rate_limited(wait));
        }
        auth::security_event(&app, &id, "sign_in_failed", Some(ip), ua, None).await;
        return Err(ApiError::forbidden(
            "That username and password don't match.",
        ));
    }
    if disabled {
        return Err(ApiError::forbidden(
            "This account is turned off. Ask your Familiar admin.",
        ));
    }
    // The key opens now, while the password is at hand, but only counts as unlocked
    // once the authenticator code is right too.
    let opened = open_keys(&app, &id, &plain).await;
    if has_totp {
        return Err(totp_challenge(&app, &id, opened).await?);
    }
    let recovery = match opened {
        Some(o) => unlock(&app, &id, o, Some(ip), ua.clone()).await,
        None => None,
    };
    signed_in(&app, &id, ip, ua, recovery).await
}

/// Lock the account when `failures` crossed the threshold; returns seconds to wait.
async fn lock_if_needed(app: &App, user_id: &str, failures: i64, t: i64) -> ApiResult<Option<u64>> {
    if failures < FAILURES_BEFORE_LOCK {
        return Ok(None);
    }
    // 15 minutes, doubling per further failure, capped at a day.
    let exp = u32::try_from(failures - FAILURES_BEFORE_LOCK)
        .unwrap_or(10)
        .min(7);
    let until = t + (900i64 << exp).min(86_400);
    let uid = user_id.to_owned();
    app.db
        .call(move |c| {
            c.execute(
                "UPDATE users SET locked_until = MAX(locked_until, ?2) WHERE id = ?1",
                rusqlite::params![uid, until],
            )
        })
        .await?;
    Ok(Some(u64::try_from(until - t).unwrap_or(900)))
}

/// Start the second sign-in step: a 5-minute challenge for the authenticator code.
/// `opened` waits in memory for the code.
async fn totp_challenge(app: &App, user_id: &str, opened: Option<Opened>) -> ApiResult<ApiError> {
    let challenge = crypto::random_id::<24>();
    let hash = crypto::sha256(challenge.as_bytes());
    if let Some(o) = opened {
        app.fleet.keys().hold(hash.clone(), user_id, o);
    }
    let uid = user_id.to_owned();
    let t = now();
    app.db
        .call(move |c| {
            c.execute("DELETE FROM login_challenges WHERE expires_at < ?1", [t])?;
            c.execute(
                "INSERT INTO login_challenges (id_hash, user_id, expires_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![hash, uid, t + 300],
            )
        })
        .await?;
    let mut e = ApiError::unauthenticated().with("challenge", challenge);
    e.code = "totp_required";
    e.message = "Enter the 6-digit code from your authenticator app.".into();
    Ok(e)
}

#[derive(Deserialize)]
pub struct TotpBody {
    challenge: String,
    code: String,
}

/// `POST /api/login/totp`.
pub async fn login_totp(
    State(app): State<App>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    Extension(UserAgent(ua)): Extension<UserAgent>,
    Json(body): Json<TotpBody>,
) -> ApiResult<Response> {
    app.limiter
        .hit("login_ip", &ratelimit::ip_key(ip), ratelimit::LOGIN_IP)
        .map_err(ApiError::rate_limited)?;
    let hash = crypto::sha256(body.challenge.as_bytes());
    let t = now();
    let row: Option<(String, i64, Option<Vec<u8>>, i64)> = app
        .db
        .call(move |c| {
            let row = c
                .query_row(
                    "SELECT l.user_id, l.attempts, u.totp_secret, u.totp_last_step FROM login_challenges l
                     JOIN users u ON u.id = l.user_id WHERE l.id_hash = ?1 AND l.expires_at > ?2 AND u.disabled = 0",
                    rusqlite::params![hash, t],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            if row.is_some() {
                c.execute("UPDATE login_challenges SET attempts = attempts + 1 WHERE id_hash = ?1", [&hash])?;
            }
            Ok(row)
        })
        .await?;
    let expired = || ApiError::unauthenticated().with("restart", "true".into());
    let Some((uid, attempts, Some(sealed), last_step)) = row else {
        let mut e = expired();
        e.message = "That sign-in took too long. Enter your password again.".into();
        return Err(e);
    };
    if attempts >= 5 {
        let mut e = expired();
        e.message = "Too many wrong codes. Enter your password again.".into();
        return Err(e);
    }
    let secret = app.key.open(&format!("totp:{uid}"), &sealed)?;
    let wrong = || {
        ApiError::invalid(
            "code",
            "That code isn't right. Check the time on your phone and try the newest code.",
        )
    };
    let Some(step) = totp::verify(&secret, &body.code, t, last_step) else {
        // Wrong codes count toward the same lockout as wrong passwords.
        let u2 = uid.clone();
        let failures: i64 = app
            .db
            .call(move |c| c.query_row("UPDATE users SET failed_logins = failed_logins + 1 WHERE id = ?1 RETURNING failed_logins", [u2], |r| r.get(0)))
            .await?;
        if let Some(wait) = lock_if_needed(&app, &uid, failures, t).await? {
            auth::security_event(
                &app,
                &uid,
                "locked_out",
                Some(ip),
                ua,
                Some("Wrong authenticator codes".into()),
            )
            .await;
            return Err(ApiError::rate_limited(wait));
        }
        auth::security_event(
            &app,
            &uid,
            "sign_in_failed",
            Some(ip),
            ua,
            Some("Wrong authenticator code".into()),
        )
        .await;
        return Err(wrong());
    };
    let (u2, chash) = (uid.clone(), crypto::sha256(body.challenge.as_bytes()));
    let claimed = app
        .db
        .call(move |c| {
            // Only one request can use a time step, even when two arrive at once.
            let n = c.execute(
                "UPDATE users SET totp_last_step = ?2 WHERE id = ?1 AND totp_last_step < ?2",
                rusqlite::params![u2, step],
            )?;
            c.execute("DELETE FROM login_challenges WHERE id_hash = ?1", [chash])?;
            Ok(n == 1)
        })
        .await?;
    if !claimed {
        return Err(wrong());
    }
    let held = app
        .fleet
        .keys()
        .take(&crypto::sha256(body.challenge.as_bytes()), &uid);
    let recovery = match held {
        Some(o) => unlock(&app, &uid, o, Some(ip), ua.clone()).await,
        // Lost to a restart between the two steps: signed in, but still locked.
        None => None,
    };
    signed_in(&app, &uid, ip, ua, recovery).await
}

/// `POST /api/logout`.
pub async fn logout(State(app): State<App>, MaybeSession(s): MaybeSession) -> ApiResult<Response> {
    if let Some(s) = s {
        let hash = s.id_hash.clone();
        app.db
            .call(move |c| c.execute("DELETE FROM sessions WHERE id_hash = ?1", [hash]))
            .await?;
        auth::security_event(&app, &s.user.id, "sign_out", Some(s.ip), s.user_agent, None).await;
    }
    let mut r = Json(json!({})).into_response();
    if let Ok(v) = HeaderValue::from_str(&auth::session_cookie(app.config.secure_cookies(), "")) {
        r.headers_mut().insert(header::SET_COOKIE, v);
    }
    Ok(r)
}

struct Invite {
    id: String,
    role: String,
    reset_user: Option<String>,
    expires_at: i64,
}

async fn load_invite(app: &App, code: &str) -> ApiResult<Invite> {
    let hash = crypto::sha256(code.as_bytes());
    let t = now();
    app.db
        .call(move |c| {
            c.query_row(
                "SELECT id, role, reset_user, expires_at FROM invites WHERE code_hash = ?1 AND used_at IS NULL AND expires_at > ?2",
                rusqlite::params![hash, t],
                |r| Ok(Invite { id: r.get(0)?, role: r.get(1)?, reset_user: r.get(2)?, expires_at: r.get(3)? }),
            )
            .optional()
        })
        .await?
        .ok_or_else(|| ApiError::not_found("invite (it may have been used or expired; ask for a new link)"))
}

/// `GET /api/invites/:code`.
pub async fn invite_info(
    State(app): State<App>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    Path(code): Path<String>,
) -> ApiResult<Json<Value>> {
    app.limiter
        .hit("login_ip", &ratelimit::ip_key(ip), ratelimit::LOGIN_IP)
        .map_err(ApiError::rate_limited)?;
    let inv = load_invite(&app, &code).await?;
    let (username, encrypted) = match &inv.reset_user {
        Some(uid) => {
            let u2 = uid.clone();
            let name = app
                .db
                .call(move |c| {
                    c.query_row("SELECT username FROM users WHERE id = ?1", [u2], |r| {
                        r.get::<_, String>(0)
                    })
                    .optional()
                })
                .await?;
            (name, encrypted_macs(&app, uid).await?)
        }
        None => (None, Vec::new()),
    };
    Ok(Json(json!({
        "role": inv.role,
        "expires_at": crate::db::rfc3339(inv.expires_at),
        "reset_username": username,
        // A new password can't open the old key: the recovery key keeps these Macs.
        "needs_recovery_key": !encrypted.is_empty(),
        "encrypted_macs": encrypted.len(),
    })))
}

/// A person's encrypted Macs (only those need their old key).
async fn encrypted_macs(app: &App, user_id: &str) -> ApiResult<Vec<String>> {
    let uid = user_id.to_owned();
    Ok(app
        .db
        .call(move |c| {
            let mut st = c.prepare(
                "SELECT id FROM instances WHERE user_id = ?1 AND encrypted = 1 AND is_builder = 0",
            )?;
            let rows = st.query_map([uid], |r| r.get(0))?;
            rows.collect()
        })
        .await?)
}

/// The data key a reset password gets, and what happened to the old one.
struct ResetKeys {
    key: DataKey,
    stored: Stored,
    /// A new recovery key to show once.
    recovery_key: Option<String>,
    /// The old key is gone: these encrypted Macs can't be read any more.
    discarded: Vec<String>,
    used_recovery_key: bool,
}

/// Work out the data key after a reset link: the recovery key opens the old one; or,
/// when the person has none and agreed, a new key replaces it and their encrypted Macs
/// are erased; or, with nothing encrypted, a new key quietly replaces it.
async fn reset_keys(
    app: &App,
    user_id: &str,
    password: &Zeroizing<String>,
    recovery_key: Option<&str>,
    discard: bool,
) -> ApiResult<ResetKeys> {
    let stored = keys::load(&app.db, user_id).await?;
    let encrypted = encrypted_macs(app, user_id).await?;
    let typed = recovery_key.filter(|t| !t.trim().is_empty());
    if let (Some(stored), Some(text)) = (&stored, typed) {
        let recovery = RecoveryKey::parse(text).ok_or_else(|| {
            ApiError::invalid(
                "recovery_key",
                "That isn't a recovery key. It has 8 groups of 4 letters and digits.",
            )
        })?;
        let key = stored
            .open_with_recovery(user_id, &recovery)
            .ok_or_else(|| ApiError::invalid("recovery_key", "That recovery key isn't right."))?;
        let (st, uid, pw, k2) = (
            stored.clone(),
            user_id.to_owned(),
            password.clone(),
            key.clone(),
        );
        let next = tokio::task::spawn_blocking(move || st.with_password(&uid, &pw, &k2))
            .await
            .map_err(ApiError::internal)??;
        return Ok(ResetKeys {
            key,
            stored: next,
            recovery_key: None,
            discarded: Vec::new(),
            used_recovery_key: true,
        });
    }
    if stored.is_some() && !encrypted.is_empty() && !discard {
        let mut e = ApiError::conflict(format!(
            "Enter your recovery key to keep the data on your encrypted Mac{}.",
            if encrypted.len() == 1 { "" } else { "s" }
        ));
        e.code = "recovery_key_required";
        return Err(e);
    }
    let key = DataKey::random();
    let recovery = RecoveryKey::random();
    let shown = recovery.display();
    let (uid, pw, k2) = (user_id.to_owned(), password.clone(), key.clone());
    let next = tokio::task::spawn_blocking(move || Stored::seal(&uid, &pw, &k2, &recovery))
        .await
        .map_err(ApiError::internal)??;
    Ok(ResetKeys {
        key,
        stored: next,
        recovery_key: Some(shown),
        discarded: if stored.is_some() {
            encrypted
        } else {
            Vec::new()
        },
        used_recovery_key: false,
    })
}

#[derive(Deserialize)]
pub struct AcceptBody {
    #[serde(default)]
    username: String,
    #[serde(default)]
    display_name: String,
    password: String,
    /// Reset links: keeps the person's encrypted Macs.
    #[serde(default)]
    recovery_key: Option<String>,
    /// Reset links without the recovery key: erase their encrypted Macs and start over.
    #[serde(default)]
    discard_encrypted_macs: bool,
}

/// `POST /api/invites/:code/accept`: join, or set a new password for a reset link.
pub async fn invite_accept(
    State(app): State<App>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    Extension(UserAgent(ua)): Extension<UserAgent>,
    Path(code): Path<String>,
    Json(body): Json<AcceptBody>,
) -> ApiResult<Response> {
    app.limiter
        .hit("login_ip", &ratelimit::ip_key(ip), ratelimit::LOGIN_IP)
        .map_err(ApiError::rate_limited)?;
    let inv = load_invite(&app, &code).await?;
    if let Some(uid) = inv.reset_user.clone() {
        let u2 = uid.clone();
        let username: String = app
            .db
            .call(move |c| {
                c.query_row("SELECT username FROM users WHERE id = ?1", [u2], |r| {
                    r.get(0)
                })
            })
            .await?;
        password::check_policy(&body.password, &username)
            .map_err(|m| ApiError::invalid("password", m))?;
        let plain = Zeroizing::new(body.password.clone());
        // The old password wrapped the data key; the new one can't open it.
        let reset = reset_keys(
            &app,
            &uid,
            &plain,
            body.recovery_key.as_deref(),
            body.discard_encrypted_macs,
        )
        .await?;
        let hash = password::hash(body.password).await?;
        let (u3, iid, stored, gone) = (
            uid.clone(),
            inv.id.clone(),
            reset.stored.clone(),
            reset.discarded.clone(),
        );
        let outcome = app
            .db
            .call(move |c| {
                let tx = c.transaction()?;
                let claimed = tx.execute(
                    "UPDATE invites SET used_at = ?2 WHERE id = ?1 AND used_at IS NULL",
                    rusqlite::params![iid, now()],
                )?;
                if claimed == 0 {
                    return Ok(None);
                }
                tx.execute(
                    "UPDATE users SET password_hash = ?2, failed_logins = 0, locked_until = 0 WHERE id = ?1",
                    rusqlite::params![u3, hash],
                )?;
                tx.execute("DELETE FROM sessions WHERE user_id = ?1", [&u3])?;
                stored.save(&tx, &u3)?;
                // Their data can't be decrypted any more; the files go right after.
                for id in &gone {
                    tx.execute("DELETE FROM instances WHERE id = ?1 AND user_id = ?2", [id, &u3])?;
                }
                let has_totp: bool = tx.query_row("SELECT totp_secret IS NOT NULL FROM users WHERE id = ?1", [&u3], |r| r.get(0))?;
                tx.commit()?;
                Ok(Some(has_totp))
            })
            .await?;
        let Some(has_totp) = outcome else {
            return Err(ApiError::not_found(
                "invite (it may have been used or expired; ask for a new link)",
            ));
        };
        app.fleet.keys().remove(&uid);
        for id in &reset.discarded {
            super::instances::remove_files(&app, id).await;
        }
        auth::security_event(
            &app,
            &uid,
            "password_changed",
            Some(ip),
            ua.clone(),
            Some("Reset link".into()),
        )
        .await;
        if reset.used_recovery_key {
            auth::security_event(&app, &uid, "recovery_key_used", Some(ip), ua.clone(), None).await;
        } else if !reset.discarded.is_empty() {
            let n = reset.discarded.len();
            auth::security_event(
                &app,
                &uid,
                "keys_reset",
                Some(ip),
                ua.clone(),
                Some(format!(
                    "{n} encrypted Mac{} erased",
                    if n == 1 { "" } else { "s" }
                )),
            )
            .await;
        }
        let opened = Opened {
            key: reset.key,
            recovery_key: reset.recovery_key,
        };
        // A reset link replaces the password only; the authenticator is still required,
        // so whoever made the link (an admin) can't sign in as the person.
        if has_totp {
            return Err(totp_challenge(&app, &uid, Some(opened)).await?);
        }
        let recovery = unlock(&app, &uid, opened, Some(ip), ua.clone()).await;
        return signed_in(&app, &uid, ip, ua, recovery).await;
    }
    let username = check_username(&body.username)?;
    let display = check_display(&body.display_name)?;
    password::check_policy(&body.password, &username)
        .map_err(|m| ApiError::invalid("password", m))?;
    let plain = Zeroizing::new(body.password.clone());
    let hash = password::hash(body.password).await?;
    let id = crypto::random_id::<10>();
    let (uid, iid, role) = (id.clone(), inv.id.clone(), inv.role.clone());
    let created = app
        .db
        .call(move |c| {
            let tx = c.transaction()?;
            let taken: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE username = ?1)", [&username], |r| r.get(0))?;
            if taken {
                return Ok(false);
            }
            // Claim the invite inside the transaction so it can't be used twice.
            let claimed = tx.execute("UPDATE invites SET used_at = ?2 WHERE id = ?1 AND used_at IS NULL", rusqlite::params![iid, now()])?;
            if claimed == 0 {
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO users (id, username, display_name, password_hash, role, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![uid, username, display, hash, role, now()],
            )?;
            tx.commit()?;
            Ok(true)
        })
        .await?;
    if !created {
        return Err(ApiError::invalid(
            "username",
            "That username is taken. Choose another.",
        ));
    }
    let recovery = match open_keys(&app, &id, &plain).await {
        Some(o) => unlock(&app, &id, o, Some(ip), ua.clone()).await,
        None => None,
    };
    signed_in(&app, &id, ip, ua, recovery).await
}
