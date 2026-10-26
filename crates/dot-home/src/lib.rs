//! Familiar: a self-hosted server that gives each person a private macOS VM their AI
//! assistant can drive over MCP. See docs/home/ARCHITECTURE.md.
pub mod api;
pub mod auth;
pub mod config;
pub mod crypto;
pub mod db;
pub mod error;
pub mod events;
pub mod guest;
pub mod guestdist;
pub mod host;
pub mod images;
pub mod keys;
pub mod mcp;
pub mod oauth;
pub mod packs;
pub mod push;
pub mod snapshots;
pub mod state;
pub mod storage;
pub mod tools;
pub mod updates;
pub mod util;
pub mod vm;
pub mod web;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{get, post},
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use state::App;
use std::sync::{Arc, atomic::AtomicBool};
use tower_http::limit::RequestBodyLimitLayer;

/// Build the application state: open the database, load keys, adopt running Macs.
pub async fn build(config: config::Config) -> anyhow::Result<App> {
    std::fs::create_dir_all(&config.data_dir)?;
    std::fs::create_dir_all(config.data_dir.join("images"))?;
    std::fs::create_dir_all(config.data_dir.join("instances"))?;
    std::fs::create_dir_all(config.data_dir.join("guest"))?;
    let key = crypto::MasterKey::load_or_create(&config.data_dir)?;
    let db = db::Db::open(&config.data_dir.join("dot-home.db"))?;
    let push = push::Push::load_or_create(&db, &key, config.origin()).await?;
    let config = Arc::new(config);
    let events = events::Events::default();
    let fleet = vm::Fleet::new(
        config.clone(),
        db.clone(),
        key.clone(),
        events.clone(),
        vm::Hv::Qemu(vm::qemu::Qemu {
            config: config.vm.clone(),
            data_dir: config.data_dir.clone(),
        }),
    );
    let users: i64 = db
        .call(|c| c.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0)))
        .await?;
    let require_totp = db
        .setting("require_totp")
        .await?
        .is_none_or(|v| v == "true");
    let app = Arc::new(state::Inner {
        config,
        db,
        key,
        limiter: auth::ratelimit::Limiter::default(),
        events,
        fleet,
        catalog: Arc::new(packs::Catalog::builtin()),
        approvals: tools::approvals::Waiters::default(),
        push,
        setup_required: AtomicBool::new(users == 0),
        require_totp: AtomicBool::new(require_totp),
        updates: updates::Updater::default(),
        storage: storage::Cache::default(),
        started: std::time::Instant::now(),
    });
    if users == 0 {
        print_setup_link(&app).await?;
    }
    app.fleet.adopt_all().await?;
    spawn_housekeeping(app.clone());
    snapshots::spawn_sweeper(app.clone());
    updates::spawn_health_commit(app.clone());
    updates::spawn_scheduler(app.clone());
    Ok(app)
}

/// Create (or replace) the one-time setup token and print the link.
pub async fn print_setup_link(app: &App) -> anyhow::Result<String> {
    let token = crypto::random_id::<24>();
    let hash: String = crypto::sha256(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    app.db.set_setting("setup_token_hash", hash).await?;
    let url = format!("{}/setup?token={token}", app.config.origin());
    crypto::write_private(&app.config.data_dir.join("setup-url.txt"), url.as_bytes())?;
    tracing::warn!("No accounts yet. Open this link to create the first admin: {url}");
    Ok(url)
}

/// Hourly cleanup: expired sessions, challenges, codes and requests; old activity.
fn spawn_housekeeping(app: App) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            tick.tick().await;
            let days = i64::from(app.config.limits.activity_days);
            let t = db::now();
            let result = app
                .db
                .call(move |c| {
                    c.execute("DELETE FROM sessions WHERE expires_at < ?1", [t])?;
                    c.execute("DELETE FROM login_challenges WHERE expires_at < ?1", [t])?;
                    c.execute("DELETE FROM oauth_codes WHERE expires_at < ?1", [t - 3600])?;
                    c.execute("DELETE FROM oauth_requests WHERE expires_at < ?1", [t])?;
                    c.execute(
                        "DELETE FROM tokens WHERE expires_at IS NOT NULL AND expires_at < ?1",
                        [t],
                    )?;
                    c.execute("DELETE FROM activity WHERE at < ?1", [t - days * 86_400])?;
                    c.execute(
                        "DELETE FROM approvals WHERE expires_at < ?1",
                        [t - 30 * 86_400],
                    )?;
                    c.execute(
                        "DELETE FROM receipts WHERE created_at < ?1",
                        [t - 30 * 86_400],
                    )?;
                    c.execute(
                        "DELETE FROM invites WHERE expires_at < ?1",
                        [t - 30 * 86_400],
                    )?;
                    c.execute_batch("PRAGMA optimize;")
                })
                .await;
            if let Err(e) = result {
                tracing::warn!(error = %e, "housekeeping");
            }
        }
    });
}

/// Every route.
pub fn router(app: App) -> Router {
    let https = app.config.public_url.scheme() == "https";
    let mcp_app = app.clone();
    let mcp = StreamableHttpService::new(
        move || Ok(mcp::Handler::new(mcp_app.clone())),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_json_response(true)
            .with_max_request_body_bytes(4 * 1024 * 1024)
            .with_allowed_hosts({
                let host = app
                    .config
                    .public_url
                    .host_str()
                    .unwrap_or("localhost")
                    .to_owned();
                let mut hosts = vec![host.clone()];
                if let Some(port) = app.config.public_url.port() {
                    hosts.push(format!("{host}:{port}"));
                }
                hosts
            }),
    );
    let mcp = Router::new()
        .nest_service("/mcp", mcp)
        .layer(middleware::from_fn_with_state(app.clone(), oauth::mcp_gate));
    // `/api` sets its own body limits per route (uploads stream up to 2 GB), so the
    // 5 MB limit below only covers everything else.
    let api = api::router().layer(middleware::from_fn_with_state(
        app.clone(),
        auth::same_origin,
    ));
    let rest = Router::new()
        .merge(mcp)
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth::metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource",
            get(oauth::resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(oauth::resource_metadata),
        )
        .route("/oauth/register", post(oauth::register))
        .route("/oauth/authorize", get(oauth::authorize))
        .route("/oauth/token", post(oauth::token))
        .route("/oauth/revoke", post(oauth::revoke))
        .route("/guest/install.sh", get(guestdist::script))
        .route("/guest/dot-guest", get(guestdist::binary))
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .fallback(web::serve)
        .layer(RequestBodyLimitLayer::new(5 * 1024 * 1024));
    Router::new()
        .nest("/api", api)
        .merge(rest)
        .layer(middleware::from_fn(move |r, n| {
            web::security_headers(https, r, n)
        }))
        .layer(middleware::from_fn_with_state(
            app.clone(),
            auth::client_identity,
        ))
        .with_state(app)
}

async fn readyz(State(app): State<App>) -> impl IntoResponse {
    match app
        .db
        .call(|c| c.query_row("SELECT 1", [], |r| r.get::<_, i64>(0)))
        .await
    {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false })),
        ),
    }
}

/// Prometheus text, only with `Authorization: Bearer $DOT_HOME_METRICS_TOKEN`.
/// (Loopback isn't a safe signal: Macs reach the server as 127.0.0.1 through QEMU.)
async fn metrics(
    State(app): State<App>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    let expected = std::env::var("DOT_HOME_METRICS_TOKEN").unwrap_or_default();
    let given = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if expected.len() < 16
        || !bool::from(subtle::ConstantTimeEq::ct_eq(
            expected.as_bytes(),
            given.as_bytes(),
        ))
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let mut out = String::new();
    let mut by_state = std::collections::BTreeMap::new();
    for m in app.fleet.all() {
        *by_state
            .entry(format!("{:?}", m.status().state).to_lowercase())
            .or_insert(0u32) += 1;
    }
    out.push_str("# TYPE dot_home_instances gauge\n");
    for (state, n) in by_state {
        out.push_str(&format!("dot_home_instances{{state=\"{state}\"}} {n}\n"));
    }
    let (mem, running) = app.fleet.usage(None).await;
    out.push_str(&format!(
        "# TYPE dot_home_memory_used_mb gauge\ndot_home_memory_used_mb {mem}\n"
    ));
    out.push_str(&format!(
        "# TYPE dot_home_running gauge\ndot_home_running {running}\n"
    ));
    out.push_str(&format!(
        "# TYPE dot_home_uptime_seconds counter\ndot_home_uptime_seconds {}\n",
        app.started.elapsed().as_secs()
    ));
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        out,
    )
        .into_response()
}
