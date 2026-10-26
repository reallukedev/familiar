//! `dot-home`: run the server, check the host, and recover access.
use anyhow::Context;
use clap::{Parser, Subcommand};
use dot_home::config::{Config, Tls};
use std::{net::SocketAddr, path::PathBuf, time::Duration};

#[derive(Parser)]
#[command(version = dot_home::updates::build::version(), about = "Familiar: a private Mac for each person, driven by their AI assistant")]
struct Cli {
    /// Configuration file.
    #[arg(long, short, env = "DOT_HOME_CONFIG", global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server (the default).
    Serve,
    /// Check KVM, QEMU, OSX-KVM, the base image, disk space and HTTPS.
    Doctor,
    /// Print a new first-run setup link (only while there are no accounts).
    SetupLink,
    /// Print a one-time password reset link for a user.
    ResetPassword {
        /// Username.
        username: String,
        /// Also remove their two-factor setup (lost phone). They must enroll again.
        #[arg(long)]
        clear_totp: bool,
    },
    /// Exit 0 when the local server answers /healthz (for container health checks).
    Health,
    /// Make an update-signing key pair (for the release pipeline's secrets).
    Keygen,
    /// Sign a manifest with the private key in $DOT_UPDATE_SIGNING_KEY; prints the signature.
    SignManifest {
        /// manifest.json
        manifest: PathBuf,
    },
    /// Copy the database consistently (VM disks are backed up separately).
    Backup {
        /// Destination file.
        to: PathBuf,
    },
}

fn logging(json: bool) {
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};
    let filter = EnvFilter::try_from_env("DOT_HOME_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,rmcp=warn,hyper=warn"));
    let registry = tracing_subscriber::registry().with(filter);
    if json {
        registry.with(fmt::layer().json().with_target(false)).init();
    } else {
        registry.with(fmt::layer().with_target(false)).init();
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // The release pipeline runs these with no server configuration.
    match cli.command {
        Some(Command::Keygen) => {
            let (private, public) = dot_home::updates::keygen();
            println!("DOT_UPDATE_SIGNING_KEY (keep secret, add to CI secrets): {private}");
            println!("DOT_UPDATE_PUBLIC_KEYS (build-time, public): {public}");
            return Ok(());
        }
        Some(Command::SignManifest { manifest }) => {
            let key =
                std::env::var("DOT_UPDATE_SIGNING_KEY").context("set DOT_UPDATE_SIGNING_KEY")?;
            let bytes = std::fs::read(&manifest)?;
            println!("{}", dot_home::updates::sign_manifest(&bytes, &key)?);
            return Ok(());
        }
        _ => {}
    }
    let config = Config::load(cli.config.as_deref())?;
    logging(config.json_logs);
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => {
            dot_home::updates::launch(&config.data_dir, &std::env::args().collect::<Vec<_>>());
            serve(config).await
        }
        Command::Keygen | Command::SignManifest { .. } => unreachable!("handled above"),
        Command::Health => {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let port = config.bind.port();
            let check = async {
                let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
                s.write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\n\r\n")
                    .await?;
                let mut buf = Vec::new();
                s.read_to_end(&mut buf).await?;
                anyhow::Ok(buf.starts_with(b"HTTP/1.0 200") || buf.starts_with(b"HTTP/1.1 200"))
            };
            // TLS listeners don't answer plain HTTP; a completed TCP connect is good enough there.
            let ok = match config.tls {
                Tls::Off => tokio::time::timeout(Duration::from_secs(3), check)
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or(false),
                _ => tokio::net::TcpStream::connect(("127.0.0.1", port))
                    .await
                    .is_ok(),
            };
            std::process::exit(i32::from(!ok));
        }
        Command::Doctor => {
            let checks = dot_home::host::checks(&config, config.limits.min_free_disk_gb).await;
            let mut failed = 0;
            for c in &checks {
                println!(
                    "{} {:<26} {}",
                    if c.ok { "ok  " } else { "FAIL" },
                    c.label,
                    c.detail
                );
                failed += usize::from(!c.ok);
            }
            if failed > 0 {
                std::process::exit(1);
            }
            Ok(())
        }
        Command::SetupLink => {
            let db = dot_home::db::Db::open(&config.data_dir.join("dot-home.db"))?;
            let users: i64 = db
                .call(|c| c.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0)))
                .await?;
            anyhow::ensure!(
                users == 0,
                "accounts already exist; use reset-password instead"
            );
            drop(db);
            let app = dot_home::build(config).await?;
            println!("{}", dot_home::print_setup_link(&app).await?);
            Ok(())
        }
        Command::ResetPassword {
            username,
            clear_totp,
        } => {
            let db = dot_home::db::Db::open(&config.data_dir.join("dot-home.db"))?;
            let code = dot_home::crypto::random_id::<24>();
            let hash = dot_home::crypto::sha256(code.as_bytes());
            let id = dot_home::crypto::random_id::<8>();
            let uname = username.to_ascii_lowercase();
            let t = dot_home::db::now();
            let found = db
                .call(move |c| {
                    let user: Option<(String, String)> = rusqlite::OptionalExtension::optional(c.query_row(
                        "SELECT id, role FROM users WHERE username = ?1",
                        [uname],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    ))?;
                    let Some((uid, role)) = user else { return Ok(false) };
                    c.execute(
                        "INSERT INTO invites (id, code_hash, role, reset_user, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        rusqlite::params![id, hash, role, uid, t, t + 86_400],
                    )?;
                    // Also lift any lockout so the person can sign in right after.
                    c.execute("UPDATE users SET failed_logins = 0, locked_until = 0 WHERE id = ?1", [&uid])?;
                    if clear_totp {
                        c.execute("UPDATE users SET totp_secret = NULL, totp_pending = NULL WHERE id = ?1", [&uid])?;
                    }
                    Ok(true)
                })
                .await?;
            anyhow::ensure!(found, "no user named {username}");
            println!("{}/invite/{code}", config.origin());
            eprintln!(
                "Their encrypted Macs need their recovery key on that page; without it, those Macs are erased."
            );
            Ok(())
        }
        Command::Backup { to } => {
            let db = dot_home::db::Db::open(&config.data_dir.join("dot-home.db"))?;
            db.backup(to.clone()).await?;
            println!(
                "Database copied to {}. Also back up master.key; without it, two-factor secrets and Mac pairings can't be read.",
                to.display()
            );
            Ok(())
        }
    }
}

async fn serve(config: Config) -> anyhow::Result<()> {
    let bind: SocketAddr = config.bind;
    let tls = config.tls.clone();
    let data_dir = config.data_dir.clone();
    let host = config.public_url.host_str().unwrap_or_default().to_owned();
    let app = dot_home::build(config).await?;
    let router = dot_home::router(app.clone()).into_make_service_with_connect_info::<SocketAddr>();
    let handle = axum_server::Handle::new();
    tokio::spawn({
        let handle = handle.clone();
        async move {
            shutdown_signal().await;
            tracing::info!("shutting down; Macs keep running");
            handle.graceful_shutdown(Some(Duration::from_secs(20)));
        }
    });
    tracing::info!(%bind, version = dot_home::updates::build::version(), "Familiar listening");
    tokio::spawn({
        let handle = handle.clone();
        let app = app.clone();
        async move {
            app.updates.restart.notified().await;
            tracing::info!("restarting for an update; Macs keep running");
            handle.graceful_shutdown(Some(Duration::from_secs(10)));
        }
    });
    match tls {
        Tls::Off => axum_server::bind(bind).handle(handle).serve(router).await?,
        Tls::Files { cert, key } => {
            let rustls = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key)
                .await
                .with_context(|| format!("loading {} and {}", cert.display(), key.display()))?;
            // Reload the certificate daily so renewals by another tool are picked up.
            let reload = rustls.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(86_400));
                tick.tick().await;
                loop {
                    tick.tick().await;
                    if let Err(e) = reload.reload_from_pem_file(&cert, &key).await {
                        tracing::warn!(error = %e, "certificate reload failed");
                    }
                }
            });
            axum_server::bind_rustls(bind, rustls)
                .handle(handle)
                .serve(router)
                .await?;
        }
        Tls::Acme { email, staging } => {
            use futures_util::StreamExt;
            let mut state = rustls_acme::AcmeConfig::new([host])
                .contact_push(format!("mailto:{email}"))
                .cache(rustls_acme::caches::DirCache::new(data_dir.join("acme")))
                .directory_lets_encrypt(!staging)
                .state();
            let acceptor = state.axum_acceptor(state.default_rustls_config());
            tokio::spawn(async move {
                while let Some(event) = state.next().await {
                    match event {
                        Ok(ok) => tracing::info!(event = ?ok, "certificate"),
                        Err(err) => tracing::error!(error = %err, "certificate"),
                    }
                }
            });
            axum_server::bind(bind)
                .acceptor(acceptor)
                .handle(handle)
                .serve(router)
                .await?;
        }
    }
    let restarting = app
        .updates
        .restart_requested
        .load(std::sync::atomic::Ordering::SeqCst);
    app.fleet.shutdown(restarting).await;
    if restarting {
        dot_home::updates::relaunch(&std::env::args().collect::<Vec<_>>());
    }
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! { () = ctrl_c => {}, () = term => {} }
}
