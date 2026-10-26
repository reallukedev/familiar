//! `dot-guest`: the Dot Home agent inside each Mac.
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use dot_guest::config;
use dot_guest::layout::Layout;
use dot_guest::ops::{State, info};
use dot_guest::supervise::{self, Supervisor};
use dot_guest::{doctor, launchd, prepare, server, util};
use std::ffi::OsString;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "dot-guest",
    version = dot_guest::VERSION,
    about = "The Dot Home agent that runs inside each Mac"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
    /// More detailed logs.
    #[arg(long, short, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the agent (started by `supervise`, or directly by older installs).
    Run {
        /// Configuration file; otherwise the DOTCONFIG disk, then the paired configuration.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Listen on this port instead of the configured one.
        #[arg(long)]
        port: Option<u16>,
        /// Listen on this address.
        #[arg(long, default_value = "0.0.0.0")]
        bind: IpAddr,
    },
    /// Start `run` from the current version and keep it running, rolling back an update
    /// that fails to come up (what the LaunchAgent starts).
    Supervise {
        /// Passed on to `dot-guest run`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    /// Pair this Mac with Dot Home using the string from the portal, and install the agent.
    Pair {
        /// `dotpair:v1:...` from the portal.
        pairing: String,
        /// Print what would happen without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Copy this binary into the support folder and install (or restart) the LaunchAgent.
    Install {
        /// Print what would happen without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove the LaunchAgent. The configuration and installed versions are kept.
    Uninstall {
        /// Also remove the support folder: every installed version and the configuration.
        #[arg(long)]
        purge: bool,
        /// Print what would happen without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Check permissions and setup, with a fix for each problem.
    Doctor,
    /// One-time setup of the golden macOS image (run inside the VM).
    PrepareImage {
        /// Print the steps without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
}

/// Truncate the agent log once it passes this size (launchd opens it for append).
const MAX_LOG_BYTES: u64 = 20 * 1024 * 1024;

fn init_logging(verbose: bool) {
    let level = if verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };
    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_target(false)
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .init();
}

fn trim_log(home: &Path) {
    let log = launchd::Paths::new(home).log_file;
    if std::fs::metadata(&log).is_ok_and(|m| m.len() > MAX_LOG_BYTES)
        && let Ok(file) = std::fs::OpenOptions::new().write(true).open(&log)
    {
        let _ = file.set_len(0);
    }
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut int)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        std::future::pending::<()>().await;
        return;
    };
    tokio::select! {
        _ = term.recv() => tracing::info!("received SIGTERM"),
        _ = int.recv() => tracing::info!("received SIGINT"),
    }
}

async fn bind(addr: IpAddr, port: u16) -> Result<tokio::net::TcpListener> {
    let mut attempt = 0;
    loop {
        match tokio::net::TcpListener::bind((addr, port)).await {
            Ok(listener) => return Ok(listener),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && attempt < 12 => {
                attempt += 1;
                tracing::warn!(
                    port,
                    "port is in use (another dot-guest still stopping?); retrying in 5 s"
                );
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            Err(e) => {
                return Err(e).with_context(|| format!("could not listen on {addr}:{port}"));
            }
        }
    }
}

/// How the daemon ended.
enum Stopped {
    /// Asked to stop.
    Signal,
    /// An update was installed; the supervisor should start the new version.
    Restart,
}

async fn run_daemon(
    config_flag: Option<PathBuf>,
    port: Option<u16>,
    addr: IpAddr,
) -> Result<Stopped> {
    let home = util::home_dir();
    trim_log(&home);
    let (config, source) =
        match config::resolve(config_flag.as_deref(), &home, config::VOLUME_WAIT).await {
            Ok(found) => found,
            Err(e) => {
                tracing::error!("{e:#}");
                // launchd restarts us; don't spin.
                tokio::time::sleep(Duration::from_secs(10)).await;
                return Err(e);
            }
        };
    let port = port.unwrap_or(config.port);
    let supervised = supervise::is_supervised();
    tracing::info!(
        version = dot_guest::VERSION,
        supervised,
        instance = %config.instance_id,
        config = %source.path().display(),
        port,
        "starting"
    );
    let listener = bind(addr, port).await?;
    tracing::info!(%addr, port, "listening");
    let mut state = State::new(home, port);
    state.supervised = supervised;
    let state = Arc::new(state);
    let _healthy = dot_guest::ops::update::mark_healthy_later(state.clone());
    let probe_state = state.clone();
    tokio::spawn(async move {
        // Let the login session settle before asking macOS anything.
        tokio::time::sleep(Duration::from_secs(3)).await;
        info::run_probes(&probe_state).await;
        let _ = info::cached(&probe_state, Duration::from_secs(10)).await;
    });
    let restart = state.restart.clone();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let stop = async move {
        let why = tokio::select! {
            () = shutdown_signal() => Stopped::Signal,
            () = restart.notified() => Stopped::Restart,
        };
        let _ = stop_tx.send(why);
    };
    server::serve(listener, config, state, stop).await;
    let why = stop_rx.await.unwrap_or(Stopped::Signal);
    if matches!(why, Stopped::Restart) {
        tracing::info!("stopped to restart into the update");
    } else {
        tracing::info!("stopped");
    }
    Ok(why)
}

async fn supervise(args: Vec<OsString>, verbose: bool) -> Result<()> {
    let own_binary = launchd::current_exe()?;
    let mut child_args = vec![OsString::from("run")];
    child_args.extend(args);
    if verbose {
        child_args.push("--verbose".into());
    }
    let supervisor = Supervisor {
        layout: Layout::new(&util::home_dir()),
        own_binary,
        own_version: dot_guest::VERSION.to_owned(),
        child_args,
    };
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = stop_tx.send(true);
    });
    tracing::info!(version = dot_guest::VERSION, "supervisor starting");
    supervisor.run(stop_rx).await;
    tracing::info!("supervisor stopped");
    Ok(())
}

fn uid() -> u32 {
    rustix::process::getuid().as_raw()
}

fn pair(pairing: &str, dry_run: bool) -> Result<()> {
    let config = config::parse_pairing(pairing)?;
    let home = util::home_dir();
    let exe = launchd::current_exe()?;
    let path = config::user_config_path(&home);
    println!(
        "Pairing as instance {} on port {}.",
        config.instance_id, config.port
    );
    if dry_run {
        println!(
            "write          {} (mode 0600, secret not shown)",
            path.display()
        );
    } else {
        let written = config::write_user_config(&home, &config)?;
        println!("Saved {}", written.display());
    }
    install(&exe, &home, dry_run)?;
    if !dry_run {
        println!(
            "\nPaired. Grant the permissions `dot-guest doctor` lists, then check the portal."
        );
    }
    Ok(())
}

fn install(exe: &Path, home: &Path, dry_run: bool) -> Result<()> {
    let paths = launchd::Paths::new(home);
    let layout = Layout::new(home);
    let steps = launchd::install_plan(exe, dot_guest::VERSION, &layout, &paths, uid());
    launchd::execute(&steps, dry_run)?;
    if !dry_run {
        println!(
            "\nInstalled dot-guest {} at {}",
            dot_guest::VERSION,
            layout.launcher().display()
        );
    }
    Ok(())
}

fn check_config_present(home: &Path) {
    if config::find_volume_config().is_none() && !config::user_config_path(home).is_file() {
        eprintln!(
            "note: no configuration yet. On a Dot Home VM the server attaches it; on another Mac run `dot-guest pair <pairing string>`."
        );
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging(cli.verbose);
    let home = util::home_dir();
    let result = match cli.command {
        Cmd::Run { config, port, bind } => {
            return match run_daemon(config, port, bind).await {
                Ok(Stopped::Signal) => ExitCode::SUCCESS,
                Ok(Stopped::Restart) => ExitCode::from(supervise::EXIT_RESTART),
                Err(e) => {
                    eprintln!("error: {e:#}");
                    ExitCode::FAILURE
                }
            };
        }
        Cmd::Supervise { args } => supervise(args, cli.verbose).await,
        Cmd::Pair { pairing, dry_run } => pair(&pairing, dry_run),
        Cmd::Install { dry_run } => launchd::current_exe().and_then(|exe| {
            check_config_present(&home);
            install(&exe, &home, dry_run)
        }),
        Cmd::Uninstall { purge, dry_run } => {
            let paths = launchd::Paths::new(&home);
            let layout = Layout::new(&home);
            let steps = launchd::uninstall_plan(&paths, &layout, purge, uid());
            launchd::execute(&steps, dry_run).map(|()| {
                if !dry_run && !purge {
                    println!(
                        "Removed the LaunchAgent. The configuration and installed versions in {} were kept (`--purge` removes them).",
                        layout.root.display()
                    );
                }
            })
        }
        Cmd::Doctor => {
            return if doctor::run(&home).await {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
        Cmd::PrepareImage { dry_run } => {
            // Privacy permissions belong to the launcher that launchd runs.
            let launcher = Layout::new(&home).launcher();
            return if prepare::execute(&prepare::plan(&launcher), dry_run) {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
