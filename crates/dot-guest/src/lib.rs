//! `dot-guest`: the agent that runs inside each Mac for Dot Home.
//!
//! It listens for the host on TCP, authenticates it with the shared per-instance secret
//! ([`dot_wire::handshake`]), then carries out [`dot_wire::Op`]s using only system tools
//! (`osascript`, `screencapture`, `sips`, `zsh`, ...). Every child process runs in its own
//! process group with a deadline, so a cancelled or timed-out call never leaves work
//! behind.
pub mod config;
pub mod doctor;
pub mod error;
pub mod launchd;
pub mod layout;
pub mod ops;
pub mod prepare;
pub mod proc;
pub mod script;
pub mod server;
pub mod supervise;
pub mod util;

/// The agent's version, sent to the host in the handshake and printed by `--version`.
/// Release builds set `DOT_BUILD_VERSION` (e.g. `0.4.0-nightly.20261019+abc1234`).
pub const VERSION: &str = match option_env!("DOT_BUILD_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};
