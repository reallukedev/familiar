//! Portal sign-in: cookie sessions, CSRF, re-authentication and client identity.
pub mod password;
pub mod ratelimit;
pub mod totp;

use crate::{
    crypto,
    db::now,
    error::{ApiError, ApiResult},
    state::App,
};
use axum::{
    extract::{ConnectInfo, FromRequestParts, Request, State},
    http::{HeaderMap, Method, header, request::Parts},
    middleware::Next,
    response::Response,
};
use rusqlite::OptionalExtension;
use serde::Serialize;
use std::net::{IpAddr, SocketAddr};

/// Absolute session lifetime.
pub const SESSION_DAYS: i64 = 14;
/// Sessions unused for this long end.
pub const IDLE_DAYS: i64 = 3;
/// How long a password confirmation unlocks sensitive actions.
pub const CONFIRM_SECONDS: i64 = 600;

/// The caller's address, honoring `X-Forwarded-For` only from trusted proxies.
#[derive(Debug, Clone)]
pub struct ClientIp(pub IpAddr);

/// The caller's user agent (truncated).
#[derive(Debug, Clone)]
pub struct UserAgent(pub Option<String>);

/// Middleware: resolve [`ClientIp`] and [`UserAgent`] once per request.
pub async fn client_identity(State(app): State<App>, mut request: Request, next: Next) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
        .unwrap_or(IpAddr::from([127, 0, 0, 1]));
    let trusted = |ip: IpAddr| app.config.trusted_proxies.iter().any(|n| n.contains(ip));
    let ip = if trusted(peer) {
        forwarded_ip(request.headers(), &trusted).unwrap_or(peer)
    } else {
        peer
    };
    let ua = request
        .headers()
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| crate::util::truncate(s, 200));
    request.extensions_mut().insert(ClientIp(ip));
    request.extensions_mut().insert(UserAgent(ua));
    next.run(request).await
}

/// Walk `X-Forwarded-For` from the right, skipping our own trusted proxies (e.g. Caddy,
/// then Cloudflare's edge): the first address that isn't one of them is the visitor.
/// Anything to its left was written by the visitor and can't be trusted.
fn forwarded_ip(headers: &HeaderMap, trusted: &dyn Fn(IpAddr) -> bool) -> Option<IpAddr> {
    let list = headers.get("x-forwarded-for")?.to_str().ok()?;
    let mut last = None;
    for entry in list.rsplit(',') {
        let ip: IpAddr = entry.trim().parse().ok()?;
        last = Some(ip);
        if !trusted(ip) {
            return Some(ip);
        }
    }
    last
}

/// Middleware for `/api`: changes must come from our own origin.
pub async fn same_origin(State(app): State<App>, request: Request, next: Next) -> Response {
    if is_mutation(request.method()) && !origin_ok(request.headers(), &app.config.origin()) {
        return axum::response::IntoResponse::into_response(ApiError::csrf());
    }
    next.run(request).await
}

fn is_mutation(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// `Origin` must equal ours; browsers always send it on cross-site and same-site POSTs.
pub fn origin_ok(headers: &HeaderMap, origin: &str) -> bool {
    match headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        Some(o) => o == origin,
        // Without Origin, fall back to Fetch Metadata; refuse when neither says same-origin.
        None => headers
            .get("sec-fetch-site")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == "same-origin"),
    }
}

/// Name of the session cookie.
pub fn cookie_name(secure: bool) -> &'static str {
    if secure { "__Host-dot" } else { "dot" }
}

/// `Set-Cookie` value for a new session (or an expired one when `value` is empty).
pub fn session_cookie(secure: bool, value: &str) -> String {
    let name = cookie_name(secure);
    let max_age = if value.is_empty() {
        0
    } else {
        SESSION_DAYS * 86_400
    };
    let secure_attr = if secure { "; Secure" } else { "" };
    format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure_attr}")
}

fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_owned())
}

/// Public view of a user.
#[derive(Debug, Clone, Serialize)]
pub struct UserView {
    /// Id.
    pub id: String,
    /// Login name.
    pub username: String,
    /// Shown name.
    pub display_name: String,
    /// `admin` or `member`.
    pub role: String,
    /// Has TOTP.
    pub totp_enabled: bool,
    /// RFC 3339.
    pub created_at: String,
}

impl UserView {
    /// Map a `users` row selected with [`USER_COLUMNS`].
    pub fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get(0)?,
            username: r.get(1)?,
            display_name: r.get(2)?,
            role: r.get(3)?,
            totp_enabled: r.get::<_, Option<Vec<u8>>>(4)?.is_some(),
            created_at: crate::db::rfc3339(r.get(5)?),
        })
    }
    /// Is an admin.
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }
}

/// Columns for [`UserView::from_row`].
pub const USER_COLUMNS: &str = "id, username, display_name, role, totp_secret, created_at";

/// A signed-in request. Extracting it enforces CSRF on changes.
#[derive(Debug, Clone)]
pub struct Session {
    /// Who.
    pub user: UserView,
    /// Session row key.
    pub id_hash: Vec<u8>,
    /// Shown in the sessions list.
    pub public_id: String,
    /// CSRF token.
    pub csrf: String,
    /// Re-authenticated until (unix seconds).
    pub confirmed_until: i64,
    /// Caller address.
    pub ip: IpAddr,
    /// Caller user agent.
    pub user_agent: Option<String>,
}

impl Session {
    /// Require a recent password confirmation.
    pub fn require_confirmed(&self) -> ApiResult<()> {
        if self.confirmed_until >= now() {
            Ok(())
        } else {
            Err(ApiError::reauth())
        }
    }
    /// Require the admin role.
    pub fn require_admin(&self) -> ApiResult<()> {
        if self.user.is_admin() {
            Ok(())
        } else {
            Err(ApiError::forbidden("Only an admin can do that."))
        }
    }
}

impl FromRequestParts<App> for Session {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        let secure = app.config.secure_cookies();
        let raw = read_cookie(&parts.headers, cookie_name(secure))
            .ok_or_else(ApiError::unauthenticated)?;
        let ip = parts
            .extensions
            .get::<ClientIp>()
            .map(|c| c.0)
            .unwrap_or(IpAddr::from([127, 0, 0, 1]));
        let user_agent = parts
            .extensions
            .get::<UserAgent>()
            .and_then(|u| u.0.clone());
        let session = load(app, crypto::sha256(raw.as_bytes()), ip, user_agent).await?;
        let path = parts.uri.path();
        let path = path.strip_prefix("/api").unwrap_or(path);
        let exempt = ["/account", "/session", "/logout"]
            .iter()
            .any(|p| path.starts_with(p));
        if app.require_totp.load(std::sync::atomic::Ordering::Relaxed)
            && !session.user.totp_enabled
            && !exempt
        {
            let mut e = ApiError::forbidden(
                "Set up two-factor authentication in Account before continuing.",
            );
            e.code = "totp_setup_required";
            return Err(e);
        }
        if is_mutation(&parts.method) {
            let sent = parts
                .headers
                .get("x-dot-csrf")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if !bool::from(subtle::ConstantTimeEq::ct_eq(
                sent.as_bytes(),
                session.csrf.as_bytes(),
            )) {
                return Err(ApiError::csrf());
            }
        }
        Ok(session)
    }
}

/// An optional session (for `/api/session`).
pub struct MaybeSession(pub Option<Session>);

impl FromRequestParts<App> for MaybeSession {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        match Session::from_request_parts(parts, app).await {
            Ok(s) => Ok(Self(Some(s))),
            Err(e) if e.code == "unauthenticated" => Ok(Self(None)),
            Err(e) => Err(e),
        }
    }
}

async fn load(
    app: &App,
    id_hash: Vec<u8>,
    ip: IpAddr,
    user_agent: Option<String>,
) -> ApiResult<Session> {
    let t = now();
    let key = id_hash.clone();
    let ip_text = ip.to_string();
    let row = app
        .db
        .call(move |c| {
            let row = c
                .query_row(
                    &format!(
                        "SELECT s.public_id, s.csrf, s.confirmed_until, s.last_seen_at, s.expires_at, u.disabled,
                                {}
                         FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.id_hash = ?1",
                        USER_COLUMNS
                            .split(", ")
                            .map(|c| format!("u.{c}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    [&key],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, i64>(2)?,
                            r.get::<_, i64>(3)?,
                            r.get::<_, i64>(4)?,
                            r.get::<_, bool>(5)?,
                            UserView {
                                id: r.get(6)?,
                                username: r.get(7)?,
                                display_name: r.get(8)?,
                                role: r.get(9)?,
                                totp_enabled: r.get::<_, Option<Vec<u8>>>(10)?.is_some(),
                                created_at: crate::db::rfc3339(r.get(11)?),
                            },
                        ))
                    },
                )
                .optional()?;
            let Some((public_id, csrf, confirmed, last_seen, expires, disabled, user)) = row else {
                return Ok(None);
            };
            if disabled || expires < t || last_seen < t - IDLE_DAYS * 86_400 {
                c.execute("DELETE FROM sessions WHERE id_hash = ?1", [&key])?;
                return Ok(None);
            }
            if t - last_seen > 60 {
                c.execute(
                    "UPDATE sessions SET last_seen_at = ?2, ip = ?3 WHERE id_hash = ?1",
                    (&key, t, &ip_text),
                )?;
                c.execute(
                    "UPDATE users SET last_seen_at = ?2 WHERE id = ?1",
                    (&user.id, t),
                )?;
            }
            Ok(Some((public_id, csrf, confirmed, user)))
        })
        .await?;
    let (public_id, csrf, confirmed_until, user) = row.ok_or_else(ApiError::unauthenticated)?;
    Ok(Session {
        user,
        id_hash,
        public_id,
        csrf,
        confirmed_until,
        ip,
        user_agent,
    })
}

/// Whether a session still exists for an enabled user (long-lived streams recheck this).
pub async fn still_valid(app: &App, id_hash: &[u8]) -> bool {
    let key = id_hash.to_vec();
    let t = now();
    app.db
        .call(move |c| {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions s JOIN users u ON u.id = s.user_id
                 WHERE s.id_hash = ?1 AND s.expires_at > ?2 AND u.disabled = 0)",
                rusqlite::params![key, t],
                |r| r.get(0),
            )
        })
        .await
        .unwrap_or(false)
}

/// Create a session row; returns the cookie value. Signing in counts as confirming.
pub async fn create_session(
    app: &App,
    user_id: &str,
    ip: IpAddr,
    user_agent: Option<String>,
) -> ApiResult<String> {
    let value = crypto::random_id::<32>();
    let hash = crypto::sha256(value.as_bytes());
    let public_id = crypto::random_id::<10>();
    let csrf = crypto::random_id::<24>();
    let user_id = user_id.to_owned();
    let t = now();
    app.db
        .call(move |c| {
            c.execute(
                "INSERT INTO sessions (id_hash, public_id, user_id, csrf, created_at, last_seen_at, expires_at, confirmed_until, ip, user_agent)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![hash, public_id, user_id, csrf, t, t + SESSION_DAYS * 86_400, t + CONFIRM_SECONDS, ip.to_string(), user_agent],
            )
            .map(|_| ())
        })
        .await?;
    Ok(value)
}

/// Append to a user's security log.
pub async fn security_event(
    app: &App,
    user_id: &str,
    kind: &'static str,
    ip: Option<IpAddr>,
    user_agent: Option<String>,
    detail: Option<String>,
) {
    let user_id = user_id.to_owned();
    let result = app
        .db
        .call(move |c| {
            c.execute(
                "INSERT INTO security_events (user_id, at, kind, ip, user_agent, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![user_id, now(), kind, ip.map(|i| i.to_string()), user_agent, detail],
            )?;
            // Keep the log bounded per user.
            c.execute(
                "DELETE FROM security_events WHERE user_id = ?1 AND id NOT IN
                 (SELECT id FROM security_events WHERE user_id = ?1 ORDER BY id DESC LIMIT 500)",
                [&user_id],
            )
            .map(|_| ())
        })
        .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, "security event not recorded");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn origin_cookie_and_forwarding_rules() {
        let mut h = HeaderMap::new();
        assert!(!origin_ok(&h, "https://home.example"));
        h.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        assert!(origin_ok(&h, "https://home.example"));
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        assert!(
            !origin_ok(&h, "https://home.example"),
            "Origin wins over fetch metadata"
        );
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://home.example"),
        );
        assert!(origin_ok(&h, "https://home.example"));

        let c = session_cookie(true, "abc");
        assert!(c.starts_with("__Host-dot=abc; Path=/; HttpOnly; SameSite=Lax"));
        assert!(c.ends_with("; Secure"));
        assert!(session_cookie(false, "").contains("Max-Age=0"));

        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; __Host-dot=xyz; b=2"),
        );
        assert_eq!(read_cookie(&h, "__Host-dot").as_deref(), Some("xyz"));
        h.insert(
            "x-forwarded-for",
            HeaderValue::from_static("6.6.6.6, 203.0.113.9, 104.16.0.1"),
        );
        // A visitor-forged entry, the real visitor, then Cloudflare's edge (trusted).
        let cf: crate::config::Cidr = "104.16.0.0/13".parse().unwrap();
        assert_eq!(
            forwarded_ip(&h, &|ip| cf.contains(ip)),
            Some(IpAddr::from([203, 0, 113, 9]))
        );
        assert_eq!(
            forwarded_ip(&h, &|_| false),
            Some(IpAddr::from([104, 16, 0, 1]))
        );
    }
}
