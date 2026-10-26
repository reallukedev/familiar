//! Every [`Op`] the guest understands.
use crate::error::OpResult;
use dot_wire::Op;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

pub mod apps;
pub mod exec;
pub mod files;
pub mod info;
pub mod input;
pub mod mcp;
pub mod misc;
pub mod safari;
pub mod screenshot;
pub mod sdef;
pub mod ui;
pub mod update;

/// Op names this agent supports, as [`Op::name`] reports them.
pub const CAPABILITIES: &[&str] = &[
    "ping",
    "info",
    "exec",
    "script",
    "screenshot",
    "ui_tree",
    "ui_action",
    "ui_find",
    "pointer",
    "keyboard",
    "apps",
    "open_url",
    "files",
    "clipboard",
    "shortcuts",
    "safari",
    "notify",
    "mcp",
];

/// Added to [`CAPABILITIES`] in `info` only when the supervisor can restart the agent.
pub const UPDATE_CAPABILITY: &str = "update";

/// State shared by every call on every connection.
#[derive(Debug)]
pub struct State {
    /// The signed-in user's home folder.
    pub home: PathBuf,
    /// Recent accessibility snapshots, for resolving refs.
    pub ui: std::sync::Mutex<ui::UiStore>,
    /// Serializes mouse and keyboard input so two actions never interleave.
    pub input_lock: tokio::sync::Mutex<()>,
    /// Cached description of the Mac.
    pub info: info::InfoCache,
    /// Port the daemon listens on (for the status file).
    pub port: u16,
    /// Started by `dot-guest supervise`, which restarts it into an installed update.
    pub supervised: bool,
    /// Notified when the daemon should exit so the supervisor restarts it.
    pub restart: Arc<tokio::sync::Notify>,
    /// The update transfer in progress.
    pub update: tokio::sync::Mutex<Option<update::Transfer>>,
    /// The host has connected at least once.
    pub healthy: std::sync::atomic::AtomicBool,
    /// Running MCP servers and HTTP sessions, by server id.
    pub mcp: mcp::Pool,
}

impl State {
    /// Fresh state for a daemon.
    pub fn new(home: PathBuf, port: u16) -> Self {
        Self {
            home,
            ui: std::sync::Mutex::new(ui::UiStore::default()),
            input_lock: tokio::sync::Mutex::new(()),
            info: info::InfoCache::default(),
            port,
            supervised: false,
            restart: Arc::default(),
            update: tokio::sync::Mutex::new(None),
            healthy: std::sync::atomic::AtomicBool::new(false),
            mcp: mcp::Pool::default(),
        }
    }
}

/// What one call runs with.
#[derive(Debug, Clone)]
pub struct Ctx {
    /// When the call must be finished.
    pub deadline: Instant,
    /// Shared state.
    pub state: Arc<State>,
}

impl Ctx {
    /// Milliseconds left before the deadline, minus `margin`, at least 100 ms. Passed to
    /// scripts that should stop on their own and return partial results.
    pub fn budget_ms(&self, margin: Duration) -> u64 {
        let left = self.deadline.saturating_duration_since(Instant::now());
        u64::try_from(left.saturating_sub(margin).as_millis())
            .unwrap_or(u64::MAX)
            .max(100)
    }
}

/// Serialize an op's typed result.
pub fn to_value<T: Serialize>(value: T) -> OpResult {
    serde_json::to_value(value)
        .map_err(|e| crate::error::failed(format!("Could not encode the result: {e}")))
}

/// Run one operation.
pub async fn dispatch(op: Op, ctx: &Ctx) -> OpResult {
    match op {
        Op::Ping => Ok(serde_json::json!({})),
        Op::Info => to_value(info::fresh(ctx).await),
        Op::Exec(args) => to_value(exec::exec(ctx, args).await?),
        Op::Script(args) => to_value(
            crate::script::run_user(ctx.deadline, args.language, &args.source, &args.args).await?,
        ),
        Op::Screenshot(args) => to_value(screenshot::capture(ctx, args).await?),
        Op::UiTree(args) => to_value(ui::tree(ctx, args).await?),
        Op::UiFind(args) => to_value(ui::find(ctx, args).await?),
        Op::UiAction(args) => ui::action(ctx, args).await,
        Op::Pointer(args) => input::pointer(ctx, args).await,
        Op::Keyboard(args) => input::keyboard(ctx, args).await,
        Op::Apps(args) => apps::run(ctx, args).await,
        Op::OpenUrl(args) => misc::open_url(ctx, args).await,
        Op::Files(args) => files::run(ctx, args).await,
        Op::Clipboard(args) => misc::clipboard(ctx, args).await,
        Op::Shortcuts(args) => misc::shortcuts(ctx, args).await,
        Op::Safari(args) => safari::run(ctx, args).await,
        Op::Notify(args) => misc::notify(ctx, args).await,
        Op::Update(op) => update::run(ctx, op).await,
        Op::Mcp(op) => mcp::run(ctx, op).await,
    }
}
