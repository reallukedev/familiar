//! OAuth 2.1 authorization server for assistants, and bearer checks for `/mcp`.
//!
//! Public clients only (PKCE S256 required), dynamic registration, exact redirect URI
//! matching, single-use codes, rotating refresh tokens with reuse detection. Every
//! secret is stored as a SHA-256 hash and bound to one grant, which is bound to one Mac.
use crate::{
    auth::{self, ClientIp, Session, ratelimit},
    crypto,
    db::now,
    error::{ApiError, ApiResult},
    state::App,
    tools::Caller,
};
use axum::{
    Extension, Form, Json,
    extract::{Path, Query, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use serde_json::{Value, json};

const ACCESS_TTL: i64 = 3600;
const REFRESH_TTL: i64 = 60 * 86_400;
const CODE_TTL: i64 = 120;
const REQUEST_TTL: i64 = 600;

/// `/.well-known/oauth-authorization-server`.
pub async fn metadata(State(app): State<App>) -> Json<Value> {
    let o = app.config.origin();
    Json(json!({
        "issuer": o,
        "authorization_endpoint": format!("{o}/oauth/authorize"),
        "token_endpoint": format!("{o}/oauth/token"),
        "registration_endpoint": format!("{o}/oauth/register"),
        "revocation_endpoint": format!("{o}/oauth/revoke"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": ["mac"],
        "authorization_response_iss_parameter_supported": true
    }))
}

/// `/.well-known/oauth-protected-resource`.
pub async fn resource_metadata(State(app): State<App>) -> Json<Value> {
    let o = app.config.origin();
    Json(json!({
        "resource": format!("{o}/mcp"),
        "authorization_servers": [o],
        "bearer_methods_supported": ["header"],
        "scopes_supported": ["mac"],
        "resource_name": "Familiar"
    }))
}

fn oauth_error(status: StatusCode, error: &str, description: &str) -> Response {
    let mut r = (
        status,
        Json(json!({ "error": error, "error_description": description })),
    )
        .into_response();
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

/// Redirect URIs must be https, or http on a loopback address (native apps, RFC 8252).
pub fn redirect_ok(uri: &str) -> bool {
    let Ok(u) = url::Url::parse(uri) else {
        return false;
    };
    if u.fragment().is_some() || u.username() != "" || u.password().is_some() {
        return false;
    }
    match u.scheme() {
        "https" => u.host_str().is_some(),
        "http" => matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
        _ => false,
    }
}

#[derive(Deserialize)]
pub struct Registration {
    #[serde(default)]
    client_name: Option<String>,
    #[serde(default)]
    redirect_uris: Vec<String>,
}

/// `POST /oauth/register` (RFC 7591).
pub async fn register(
    State(app): State<App>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    Json(reg): Json<Registration>,
) -> Response {
    if let Err(wait) = app
        .limiter
        .hit("register", &ratelimit::ip_key(ip), ratelimit::REGISTER_IP)
    {
        return oauth_error(
            StatusCode::TOO_MANY_REQUESTS,
            "slow_down",
            &format!("Try again in {wait} s."),
        );
    }
    if reg.redirect_uris.is_empty()
        || reg.redirect_uris.len() > 10
        || !reg
            .redirect_uris
            .iter()
            .all(|u| u.len() <= 2000 && redirect_ok(u))
    {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_redirect_uri",
            "Redirect URIs must be https, or http on localhost.",
        );
    }
    let name = reg
        .client_name
        .as_deref()
        .and_then(|n| crate::util::clean_name(n, 80))
        .unwrap_or_else(|| "An assistant".into());
    let client_id = format!("dotc_{}", crypto::random_id::<16>());
    let uris = serde_json::to_string(&reg.redirect_uris).unwrap_or_default();
    let (cid, cname, curis) = (client_id.clone(), name.clone(), uris);
    let t = now();
    let result = app
        .db
        .call(move |c| {
            // Registrations nobody ever approved are cleaned up after a day.
            c.execute(
                "DELETE FROM oauth_clients WHERE created_at < ?1 AND client_id NOT IN (SELECT client_id FROM grants WHERE client_id IS NOT NULL)",
                [t - 86_400],
            )?;
            let count: i64 = c.query_row("SELECT COUNT(*) FROM oauth_clients", [], |r| r.get(0))?;
            if count >= 1000 {
                // Make room by dropping the oldest registration nobody approved.
                let freed = c.execute(
                    "DELETE FROM oauth_clients WHERE client_id = (SELECT client_id FROM oauth_clients
                     WHERE client_id NOT IN (SELECT client_id FROM grants WHERE client_id IS NOT NULL)
                     ORDER BY created_at LIMIT 1)",
                    [],
                )?;
                if freed == 0 {
                    return Ok(false);
                }
            }
            c.execute(
                "INSERT INTO oauth_clients (client_id, client_name, redirect_uris, created_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![cid, cname, curis, t],
            )?;
            Ok(true)
        })
        .await;
    match result {
        Ok(true) => (
            StatusCode::CREATED,
            Json(json!({
                "client_id": client_id,
                "client_name": name,
                "redirect_uris": reg.redirect_uris,
                "token_endpoint_auth_method": "none",
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "client_id_issued_at": t
            })),
        )
            .into_response(),
        Ok(false) => oauth_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "Too many registered clients.",
        ),
        Err(e) => {
            tracing::error!(error = %e, "register");
            oauth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "Registration failed.",
            )
        }
    }
}

#[derive(Deserialize)]
pub struct AuthorizeQuery {
    response_type: Option<String>,
    client_id: Option<String>,
    redirect_uri: Option<String>,
    state: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    scope: Option<String>,
    resource: Option<String>,
}

fn plain_error(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        format!("This connection link isn't valid: {message}\n\nStart connecting again from your assistant."),
    )
        .into_response()
}

/// `GET /oauth/authorize`: validate, then hand off to the portal's consent page.
pub async fn authorize(
    State(app): State<App>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    Query(q): Query<AuthorizeQuery>,
) -> Response {
    if let Err(wait) = app
        .limiter
        .hit("authorize", &ratelimit::ip_key(ip), ratelimit::AUTHORIZE_IP)
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            format!("Too many requests. Try again in {wait} s."),
        )
            .into_response();
    }
    let too_long = [
        &q.state,
        &q.scope,
        &q.resource,
        &q.client_id,
        &q.redirect_uri,
        &q.code_challenge,
    ]
    .iter()
    .any(|v| v.as_ref().is_some_and(|s| s.len() > 512));
    if too_long {
        return plain_error("a parameter is too long.");
    }
    let (Some(client_id), Some(redirect_uri)) = (q.client_id.clone(), q.redirect_uri.clone())
    else {
        return plain_error("client_id and redirect_uri are required.");
    };
    let cid = client_id.clone();
    let client: Option<String> = app
        .db
        .call(move |c| {
            c.query_row(
                "SELECT redirect_uris FROM oauth_clients WHERE client_id = ?1",
                [cid],
                |r| r.get(0),
            )
            .optional()
        })
        .await
        .ok()
        .flatten();
    let Some(uris) = client else {
        return plain_error("the assistant isn't registered with this server.");
    };
    let uris: Vec<String> = serde_json::from_str(&uris).unwrap_or_default();
    if !uris.contains(&redirect_uri) {
        return plain_error("the return address doesn't match the assistant's registration.");
    }
    // Errors before the person has seen a consent screen are shown here, never redirected,
    // so this endpoint can't be used to bounce visitors to an attacker's registered URI.
    let back =
        |error: &str| plain_error(&format!("the assistant sent an invalid request ({error})."));
    if q.response_type.as_deref() != Some("code") {
        return back("unsupported_response_type");
    }
    let challenge = match (
        q.code_challenge.as_deref(),
        q.code_challenge_method.as_deref(),
    ) {
        (Some(c), Some("S256")) if (43..=128).contains(&c.len()) => c.to_owned(),
        _ => return back("invalid_request"),
    };
    if let Some(resource) = &q.resource
        && resource.trim_end_matches('/') != app.config.mcp_url()
        && resource.trim_end_matches('/') != app.config.origin()
    {
        return back("invalid_target");
    }
    let id = crypto::random_id::<24>();
    let hash = crypto::sha256(id.as_bytes());
    let (state, scope, resource, redirect_uri) = (
        q.state.clone(),
        q.scope.clone(),
        q.resource.clone(),
        redirect_uri.clone(),
    );
    let t = now();
    let stored = app
        .db
        .call(move |c| {
            c.execute("DELETE FROM oauth_requests WHERE expires_at < ?1", [t])?;
            c.execute(
                "INSERT INTO oauth_requests (id_hash, client_id, redirect_uri, state, challenge, scope, resource, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![hash, client_id, redirect_uri, state, challenge, scope, resource, t + REQUEST_TTL],
            )
        })
        .await;
    match stored {
        Ok(_) => Redirect::to(&format!("/connect?request={id}")).into_response(),
        Err(_) => back("server_error"),
    }
}

struct PendingRequest {
    client_id: String,
    client_name: String,
    redirect_uri: String,
    state: Option<String>,
    challenge: String,
    expires_at: i64,
}

async fn load_request(app: &App, id: &str) -> ApiResult<PendingRequest> {
    let hash = crypto::sha256(id.as_bytes());
    let t = now();
    app.db
        .call(move |c| {
            c.query_row(
                "SELECT r.client_id, c.client_name, r.redirect_uri, r.state, r.challenge, r.expires_at
                 FROM oauth_requests r JOIN oauth_clients c ON c.client_id = r.client_id
                 WHERE r.id_hash = ?1 AND r.expires_at > ?2",
                rusqlite::params![hash, t],
                |r| {
                    Ok(PendingRequest {
                        client_id: r.get(0)?,
                        client_name: r.get(1)?,
                        redirect_uri: r.get(2)?,
                        state: r.get(3)?,
                        challenge: r.get(4)?,
                        expires_at: r.get(5)?,
                    })
                },
            )
            .optional()
        })
        .await?
        .ok_or_else(|| ApiError::not_found("connection request (it may have expired; start again from your assistant)"))
}

/// `GET /api/oauth/requests/:id`.
pub async fn request_info(
    State(app): State<App>,
    _s: Session,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let r = load_request(&app, &id).await?;
    let host = url::Url::parse(&r.redirect_uri)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned));
    Ok(Json(json!({
        "id": id, "client_name": r.client_name, "redirect_host": host,
        "expires_at": crate::db::rfc3339(r.expires_at)
    })))
}

#[derive(Deserialize)]
pub struct Decision {
    instance_id: Option<String>,
    approve: bool,
}

/// `POST /api/oauth/requests/:id`: the owner allows or denies.
pub async fn decide(
    State(app): State<App>,
    session: Session,
    Path(id): Path<String>,
    Json(d): Json<Decision>,
) -> ApiResult<Json<Value>> {
    let r = load_request(&app, &id).await?;
    let mut target = url::Url::parse(&r.redirect_uri)
        .map_err(|_| ApiError::bad("The return address is invalid."))?;
    // Check the choice before using up the request, so a mistake can be corrected.
    let owner = if d.approve {
        let instance_id = d
            .instance_id
            .ok_or_else(|| ApiError::invalid("instance_id", "Choose which Mac to connect."))?;
        Some(crate::api::owned_instance(&app, &session, &instance_id).await?)
    } else {
        None
    };
    let hash = crypto::sha256(id.as_bytes());
    let deleted = app
        .db
        .call(move |c| c.execute("DELETE FROM oauth_requests WHERE id_hash = ?1", [hash]))
        .await?;
    if deleted == 0 {
        // Another tab answered first.
        return Err(ApiError::not_found("request"));
    }
    if let Some(owner) = owner {
        let grant_id = crypto::random_id::<10>();
        let code = crypto::random_id::<32>();
        let code_hash = crypto::sha256(code.as_bytes());
        let (gid, uid, cid, name, redirect, challenge) = (
            grant_id.clone(),
            session.user.id.clone(),
            r.client_id.clone(),
            r.client_name.clone(),
            r.redirect_uri.clone(),
            r.challenge.clone(),
        );
        let t = now();
        app.db
            .call(move |c| {
                let tx = c.transaction()?;
                tx.execute(
                    "INSERT INTO grants (id, user_id, instance_id, client_id, name, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![gid, uid, owner, cid, name, t],
                )?;
                tx.execute(
                    "INSERT INTO oauth_codes (code_hash, grant_id, redirect_uri, challenge, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![code_hash, gid, redirect, challenge, t + CODE_TTL],
                )?;
                tx.commit()
            })
            .await?;
        auth::security_event(
            &app,
            &session.user.id,
            "grant_created",
            Some(session.ip),
            session.user_agent.clone(),
            Some(r.client_name.clone()),
        )
        .await;
        target.query_pairs_mut().append_pair("code", &code);
    } else {
        target
            .query_pairs_mut()
            .append_pair("error", "access_denied");
    }
    if let Some(s) = &r.state {
        target.query_pairs_mut().append_pair("state", s);
    }
    target
        .query_pairs_mut()
        .append_pair("iss", &app.config.origin());
    Ok(Json(json!({ "redirect": target.to_string() })))
}

#[derive(Deserialize)]
pub struct TokenRequest {
    grant_type: String,
    code: Option<String>,
    redirect_uri: Option<String>,
    client_id: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
}

fn pkce_ok(verifier: &str, challenge: &str) -> bool {
    if !(43..=128).contains(&verifier.len()) {
        return false;
    }
    let computed = URL_SAFE_NO_PAD.encode(crypto::sha256(verifier.as_bytes()));
    subtle::ConstantTimeEq::ct_eq(computed.as_bytes(), challenge.as_bytes()).into()
}

async fn issue(app: &App, grant_id: &str) -> anyhow::Result<Value> {
    let access = crypto::token("dot_at_");
    let refresh = crypto::token("dot_rt_");
    let (a_hash, r_hash) = (
        crypto::sha256(access.as_bytes()),
        crypto::sha256(refresh.as_bytes()),
    );
    let gid = grant_id.to_owned();
    let t = now();
    app.db
        .call(move |c| {
            // Only the newest pair stays valid; older unused ones are swept here.
            c.execute("DELETE FROM tokens WHERE grant_id = ?1 AND kind = 'access' AND expires_at < ?2", rusqlite::params![gid, t])?;
            c.execute(
                "INSERT INTO tokens (hash, grant_id, kind, prefix, expires_at, created_at) VALUES (?1, ?2, 'access', 'dot_at_', ?3, ?4)",
                rusqlite::params![a_hash, gid, t + ACCESS_TTL, t],
            )?;
            c.execute(
                "INSERT INTO tokens (hash, grant_id, kind, prefix, expires_at, created_at) VALUES (?1, ?2, 'refresh', 'dot_rt_', ?3, ?4)",
                rusqlite::params![r_hash, gid, t + REFRESH_TTL, t],
            )
        })
        .await?;
    Ok(json!({
        "access_token": access, "token_type": "Bearer", "expires_in": ACCESS_TTL,
        "refresh_token": refresh, "scope": "mac"
    }))
}

async fn revoke_grant(app: &App, grant_id: &str) {
    let gid = grant_id.to_owned();
    let _ = app
        .db
        .call(move |c| {
            c.execute("UPDATE grants SET revoked = 1 WHERE id = ?1", [&gid])?;
            c.execute("DELETE FROM tokens WHERE grant_id = ?1", [&gid])
        })
        .await;
}

/// `POST /oauth/token`.
pub async fn token(
    State(app): State<App>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    Form(req): Form<TokenRequest>,
) -> Response {
    if app
        .limiter
        .hit("token", &ratelimit::ip_key(ip), ratelimit::TOKEN_IP)
        .is_err()
    {
        return oauth_error(
            StatusCode::TOO_MANY_REQUESTS,
            "slow_down",
            "Too many requests.",
        );
    }
    let invalid = |d: &str| oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", d);
    let Some(client_id) = req.client_id.clone() else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_client",
            "client_id is required.",
        );
    };
    let t = now();
    match req.grant_type.as_str() {
        "authorization_code" => {
            let (Some(code), Some(verifier), Some(redirect)) =
                (req.code, req.code_verifier, req.redirect_uri)
            else {
                return oauth_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "code, code_verifier and redirect_uri are required.",
                );
            };
            let hash = crypto::sha256(code.as_bytes());
            let row = app
                .db
                .call(move |c| {
                    let tx = c.transaction()?;
                    let row: Option<(String, String, String, i64, bool, Option<String>)> = tx
                        .query_row(
                            "SELECT o.grant_id, o.redirect_uri, o.challenge, o.expires_at, o.used, g.client_id
                             FROM oauth_codes o JOIN grants g ON g.id = o.grant_id WHERE o.code_hash = ?1",
                            [&hash],
                            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
                        )
                        .optional()?;
                    if row.is_some() {
                        tx.execute("UPDATE oauth_codes SET used = 1 WHERE code_hash = ?1", [&hash])?;
                    }
                    tx.commit()?;
                    Ok(row)
                })
                .await;
            let Ok(Some((grant_id, stored_redirect, challenge, expires, used, grant_client))) = row
            else {
                return invalid("The code is invalid.");
            };
            if used {
                // A replayed code means it leaked: kill everything issued from it.
                tracing::warn!(grant = %grant_id, "authorization code reused; revoking grant");
                revoke_grant(&app, &grant_id).await;
                return invalid("The code was already used.");
            }
            if expires < t
                || stored_redirect != redirect
                || grant_client.as_deref() != Some(client_id.as_str())
                || !pkce_ok(&verifier, &challenge)
            {
                return invalid("The code, client, redirect URI or verifier doesn't match.");
            }
            match issue(&app, &grant_id).await {
                Ok(body) => no_store(Json(body).into_response()),
                Err(_) => oauth_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "server_error",
                    "Try again.",
                ),
            }
        }
        "refresh_token" => {
            let Some(refresh) = req.refresh_token else {
                return oauth_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "refresh_token is required.",
                );
            };
            let hash = crypto::sha256(refresh.as_bytes());
            let row = app
                .db
                .call(move |c| {
                    let tx = c.transaction()?;
                    let row: Option<(String, i64, bool, Option<String>, bool, bool)> = tx
                        .query_row(
                            "SELECT t.grant_id, t.expires_at, t.used, g.client_id, g.revoked, u.disabled
                             FROM tokens t JOIN grants g ON g.id = t.grant_id JOIN users u ON u.id = g.user_id
                             WHERE t.hash = ?1 AND t.kind = 'refresh'",
                            [&hash],
                            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
                        )
                        .optional()?;
                    if row.is_some() {
                        tx.execute("UPDATE tokens SET used = 1 WHERE hash = ?1", [&hash])?;
                    }
                    tx.commit()?;
                    Ok(row)
                })
                .await;
            let Ok(Some((grant_id, expires, used, grant_client, revoked, disabled))) = row else {
                return invalid("The refresh token is invalid.");
            };
            if used {
                tracing::warn!(grant = %grant_id, "refresh token reused; revoking grant");
                revoke_grant(&app, &grant_id).await;
                return invalid("The refresh token was already used.");
            }
            if expires < t
                || revoked
                || disabled
                || grant_client.as_deref() != Some(client_id.as_str())
            {
                return invalid("The refresh token expired or was revoked.");
            }
            // Used refresh tokens stay (until they expire) so a replay is recognized.
            match issue(&app, &grant_id).await {
                Ok(body) => no_store(Json(body).into_response()),
                Err(_) => oauth_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "server_error",
                    "Try again.",
                ),
            }
        }
        _ => oauth_error(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "Use authorization_code or refresh_token.",
        ),
    }
}

fn no_store(mut r: Response) -> Response {
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r.headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    r
}

#[derive(Deserialize)]
pub struct RevokeRequest {
    token: String,
}

/// `POST /oauth/revoke` (RFC 7009): always 200.
pub async fn revoke(State(app): State<App>, Form(req): Form<RevokeRequest>) -> StatusCode {
    let hash = crypto::sha256(req.token.as_bytes());
    let grant: Option<String> = app
        .db
        .call(move |c| {
            c.query_row(
                "SELECT grant_id FROM tokens WHERE hash = ?1 AND kind IN ('access','refresh')",
                [hash],
                |r| r.get(0),
            )
            .optional()
        })
        .await
        .ok()
        .flatten();
    if let Some(g) = grant {
        revoke_grant(&app, &g).await;
    }
    StatusCode::OK
}

/// Resolve a bearer token to its caller.
pub async fn verify_bearer(app: &App, token: &str) -> Option<Caller> {
    if token.len() > 200 || !token.starts_with("dot_") {
        return None;
    }
    let hash = crypto::sha256(token.as_bytes());
    let t = now();
    app.db
        .call(move |c| {
            let row: Option<(String, String, String, String, Option<i64>)> = c
                .query_row(
                    "SELECT g.id, g.user_id, g.instance_id, g.name, g.last_used_at
                     FROM tokens t JOIN grants g ON g.id = t.grant_id JOIN users u ON u.id = g.user_id
                     WHERE t.hash = ?1 AND t.kind IN ('access', 'personal') AND (t.expires_at IS NULL OR t.expires_at > ?2)
                       AND g.revoked = 0 AND u.disabled = 0",
                    rusqlite::params![hash, t],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .optional()?;
            let Some((gid, user_id, instance_id, name, last)) = row else { return Ok(None) };
            if last.is_none_or(|l| t - l > 60) {
                c.execute("UPDATE grants SET last_used_at = ?2 WHERE id = ?1", rusqlite::params![gid, t])?;
            }
            Ok(Some(Caller { user_id, instance_id, client: Some(name), grant_id: Some(gid), session: None }))
        })
        .await
        .ok()
        .flatten()
}

/// Middleware for `/mcp`: bearer token required; the caller is attached to the request.
pub async fn mcp_gate(State(app): State<App>, mut request: Request, next: Next) -> Response {
    let ip = request
        .extensions()
        .get::<ClientIp>()
        .map(|c| ratelimit::ip_key(c.0))
        .unwrap_or_default();
    if let Some(origin) = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        && origin != app.config.origin()
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    if app
        .limiter
        .check("bearer_fail", &ip, ratelimit::BEARER_FAIL_IP)
        .is_err()
    {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned);
    let caller = match token {
        Some(t) => verify_bearer(&app, &t).await,
        None => None,
    };
    let Some(caller) = caller else {
        let _ = app
            .limiter
            .hit("bearer_fail", &ip, ratelimit::BEARER_FAIL_IP);
        let mut r = StatusCode::UNAUTHORIZED.into_response();
        if let Ok(v) = HeaderValue::from_str(&format!(
            "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource\"",
            app.config.origin()
        )) {
            r.headers_mut().insert(header::WWW_AUTHENTICATE, v);
        }
        return r;
    };
    request.extensions_mut().insert(caller);
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirects_and_pkce() {
        assert!(redirect_ok(
            "https://chatgpt.com/connector_platform_oauth_redirect"
        ));
        assert!(redirect_ok("https://claude.ai/api/mcp/auth_callback"));
        assert!(redirect_ok("http://127.0.0.1:33418/callback"));
        assert!(!redirect_ok("http://evil.example/cb"));
        assert!(!redirect_ok("javascript:alert(1)"));
        assert!(!redirect_ok("https://a.example/cb#frag"));
        assert!(!redirect_ok("https://user:pw@a.example/cb"));
        // RFC 7636 appendix B.
        assert!(pkce_ok(
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        ));
        assert!(!pkce_ok(
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXK",
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        ));
        assert!(!pkce_ok("short", "x"));
    }
}
