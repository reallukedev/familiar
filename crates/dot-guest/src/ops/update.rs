//! [`UpdateOp`]: receive a new agent over the link, install it next to the old one and
//! restart into it. The supervisor rolls it back if it doesn't come up.
use super::{Ctx, State};
use crate::error::{self, OpResult};
use crate::layout::{self, Layout, Trial};
use crate::proc::Command;
use base64::Engine as _;
use dot_wire::{UPDATE_CHUNK, UpdateOp};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::time::Instant;

/// Largest binary accepted.
pub const MAX_SIZE: u64 = 512 * 1024 * 1024;
/// How long `<new binary> --version` may take.
const VERSION_CHECK: Duration = Duration::from_secs(10);
/// Time for the reply frame to reach the host before the process exits.
const RESTART_DELAY: Duration = Duration::from_millis(500);

/// Hint when the agent isn't running under the supervisor.
pub const HINT_REINSTALL: &str = "The update is installed but this agent can't restart into it on its own. On the Mac, run `dot-guest install` once to switch the LaunchAgent to the launcher; future updates then restart automatically.";

/// A transfer in progress.
#[derive(Debug)]
pub struct Transfer {
    version: String,
    sha256: String,
    size: u64,
    received: u64,
    path: PathBuf,
    file: tokio::fs::File,
    hasher: Sha256,
}

/// Run one update step.
pub async fn run(ctx: &Ctx, op: UpdateOp) -> OpResult {
    let state = &ctx.state;
    let layout = Layout::new(&state.home);
    let mut slot = state.update.lock().await;
    match op {
        UpdateOp::Begin {
            version,
            sha256,
            size,
        } => {
            discard(slot.take()).await;
            *slot = Some(begin(&layout, version, sha256, size).await?);
            Ok(json!({}))
        }
        UpdateOp::Chunk { offset, data } => {
            let Some(transfer) = slot.as_mut() else {
                return Err(no_transfer());
            };
            let received = chunk(transfer, offset, &data).await?;
            Ok(json!({ "received": received }))
        }
        UpdateOp::Commit => {
            let Some(transfer) = slot.take() else {
                return Err(no_transfer());
            };
            // The lock stays held so a Begin can't clear `update/` mid-commit.
            let installed = commit(&layout, transfer).await?;
            Ok(finish(state, &installed))
        }
        UpdateOp::Abort => {
            discard(slot.take()).await;
            Ok(json!({}))
        }
    }
}

fn no_transfer() -> dot_wire::GuestError {
    error::invalid("No update is in progress. Send Begin first.")
}

fn io_failed(what: &str, path: &Path, e: &std::io::Error) -> dot_wire::GuestError {
    error::failed(format!(
        "Could not {what} {}: {e}. Nothing was installed; check the Mac's free space and try the update again.",
        path.display()
    ))
}

async fn begin(layout: &Layout, version: String, sha256: String, size: u64) -> OpResult<Transfer> {
    layout::valid_version(&version).map_err(error::invalid)?;
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(error::invalid(
            "sha256 must be the 64 hex digits of the binary's SHA-256.",
        ));
    }
    let sha256 = sha256.to_ascii_lowercase();
    if size == 0 || size > MAX_SIZE {
        return Err(error::invalid(format!(
            "size must be between 1 byte and {} MiB.",
            MAX_SIZE / (1024 * 1024)
        )));
    }
    if layout.is_bad(&version, Some(&sha256)) {
        return Err(error::invalid(format!(
            "dot-guest {version} was installed before and rolled back because it failed to start. Publish a fixed build (a new version or a different binary) and update to that."
        )));
    }
    let dir = layout.update_dir();
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|e| io_failed("create", &dir, &e))?;
    // Partials from an earlier, abandoned transfer.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.filter_map(Result::ok) {
            if entry.path().extension().is_some_and(|e| e == "partial") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let path = layout.partial(&version);
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o755)
        .open(&path)
        .await
        .map_err(|e| io_failed("create", &path, &e))?;
    tracing::info!(version, size, "receiving an update");
    Ok(Transfer {
        version,
        sha256,
        size,
        received: 0,
        path,
        file,
        hasher: Sha256::new(),
    })
}

/// Check a chunk against the transfer: it must start where the last one ended, be at
/// most [`UPDATE_CHUNK`] bytes and not run past the announced size.
pub fn check_chunk(received: u64, size: u64, offset: u64, len: usize) -> Result<(), String> {
    if offset != received {
        return Err(format!(
            "The chunk starts at byte {offset} but {received} bytes have been received. Send chunks in order, or Begin again."
        ));
    }
    if len > UPDATE_CHUNK {
        return Err(format!(
            "The chunk is {len} bytes; the most is {UPDATE_CHUNK}."
        ));
    }
    let len = u64::try_from(len).unwrap_or(u64::MAX);
    if received.saturating_add(len) > size {
        return Err(format!(
            "The chunk would bring the transfer to {} bytes, past the {size} announced in Begin.",
            received.saturating_add(len)
        ));
    }
    Ok(())
}

async fn chunk(transfer: &mut Transfer, offset: u64, data: &str) -> OpResult<u64> {
    // Base64 is 4/3 the size; refuse oversized text before decoding it.
    if data.len() > UPDATE_CHUNK.div_ceil(3) * 4 + 4 {
        return Err(error::invalid(format!(
            "The chunk is over {UPDATE_CHUNK} bytes."
        )));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| error::invalid(format!("The chunk is not valid base64: {e}")))?;
    check_chunk(transfer.received, transfer.size, offset, bytes.len()).map_err(error::invalid)?;
    transfer
        .file
        .write_all(&bytes)
        .await
        .map_err(|e| io_failed("write", &transfer.path, &e))?;
    transfer.hasher.update(&bytes);
    transfer.received += bytes.len() as u64;
    Ok(transfer.received)
}

async fn discard(transfer: Option<Transfer>) {
    if let Some(transfer) = transfer {
        drop(transfer.file);
        let _ = tokio::fs::remove_file(&transfer.path).await;
        tracing::info!(version = transfer.version, "update discarded");
    }
}

/// What a commit installed.
#[derive(Debug)]
struct Installed {
    version: String,
    previous: Option<String>,
}

async fn commit(layout: &Layout, transfer: Transfer) -> OpResult<Installed> {
    let path = transfer.path.clone();
    let result = verify_and_install(layout, transfer).await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&path).await;
    }
    result
}

async fn verify_and_install(layout: &Layout, transfer: Transfer) -> OpResult<Installed> {
    let version = &transfer.version.clone();
    let path = &transfer.path.clone();
    if transfer.received != transfer.size {
        return Err(error::invalid(format!(
            "Only {} of {} bytes arrived. Nothing was installed; send the update again.",
            transfer.received, transfer.size
        )));
    }
    transfer
        .file
        .sync_all()
        .await
        .map_err(|e| io_failed("save", path, &e))?;
    let got = crate::util::hex(&transfer.hasher.clone().finalize());
    if got != transfer.sha256 {
        return Err(error::failed(format!(
            "The update's SHA-256 is {got}, not the {} announced. Nothing was installed; send it again.",
            transfer.sha256
        )));
    }
    check_reports_version(path, version).await?;

    let dest = layout.version_binary(version);
    let dir = dest.parent().unwrap_or(layout.root.as_path());
    std::fs::create_dir_all(dir).map_err(|e| io_failed("create", dir, &e))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| io_failed("make executable", path, &e))?;
    std::fs::rename(path, &dest).map_err(|e| io_failed("move the update to", &dest, &e))?;
    let previous = layout
        .switch_to(version)
        .map_err(|e| io_failed("switch `current` to", &dest, &e))?;
    layout
        .write_trial(&Trial {
            version: version.clone(),
            sha256: Some(transfer.sha256.clone()),
            started_at: layout::now_unix(),
            attempts: 0,
        })
        .map_err(|e| io_failed("write", &layout.trial_path(), &e))?;
    if let Err(e) = layout.forget_bad(version) {
        tracing::warn!(error = %e, "could not update the list of failed versions");
    }
    let pruned = layout.prune();
    tracing::info!(version, ?previous, ?pruned, "update installed");
    Ok(Installed {
        version: version.clone(),
        previous: previous.filter(|p| p != version),
    })
}

/// Is `version` one of the whitespace-separated words of `output`? A plain substring
/// check would take `1.0` for `1.0.1`.
pub fn reports_version(output: &str, version: &str) -> bool {
    output.split_whitespace().any(|word| word == version)
}

async fn check_reports_version(path: &Path, version: &str) -> OpResult<()> {
    let out = Command::new(path, Instant::now() + VERSION_CHECK)
        .arg("--version")
        .max_output(64 * 1024)
        .run()
        .await
        .map_err(|e| {
            error::failed(format!(
                "The update could not be started to check its version: {e}. Nothing was installed; make sure it's a macOS build of dot-guest for this Mac."
            ))
        })?;
    if out.timed_out {
        return Err(error::failed(format!(
            "`dot-guest --version` from the update didn't finish within {} s. Nothing was installed.",
            VERSION_CHECK.as_secs()
        )));
    }
    let stdout = out.stdout_text();
    if !out.success() || !reports_version(&stdout, version) {
        let mut shown = format!("{} {}", stdout.trim(), out.stderr_text());
        crate::util::truncate_chars(&mut shown, 300);
        return Err(error::failed(format!(
            "The update says it is `{}`, not {version}. Nothing was installed; check that Begin named the right version.",
            shown.trim()
        )));
    }
    Ok(())
}

fn finish(state: &State, installed: &Installed) -> Value {
    if state.supervised {
        // The supervisor restarts into `current` when this process exits; the delay lets
        // the reply reach the host first.
        let restart = state.restart.clone();
        tokio::spawn(async move {
            tokio::time::sleep(RESTART_DELAY).await;
            restart.notify_one();
        });
        json!({
            "installed": installed.version,
            "previous": installed.previous,
            "restarting": true,
        })
    } else {
        // launchd would restart the same old binary, so exiting gains nothing and only
        // drops the link. Stay up; the next `dot-guest install` picks the launcher.
        tracing::warn!(
            version = installed.version,
            "installed an update but not running under the supervisor; not restarting"
        );
        json!({
            "installed": installed.version,
            "previous": installed.previous,
            "restarting": false,
            "hint": HINT_REINSTALL,
        })
    }
}

/// A version that has listened this long without crashing is healthy even if the host
/// never connected, so a server outage after an update can't roll back a good agent.
pub const HEALTHY_AFTER: Duration = Duration::from_secs(120);

/// Why a version counts as healthy.
#[derive(Debug, Clone, Copy)]
pub enum Healthy {
    /// The host completed a handshake.
    Handshake,
    /// It has listened for [`HEALTHY_AFTER`] without crashing.
    RanLongEnough,
}

/// End this version's trial so the supervisor stops counting its restarts. Only the
/// first call does anything.
pub fn mark_healthy(state: &State, why: Healthy) {
    if state
        .healthy
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        return;
    }
    let layout = Layout::new(&state.home);
    if layout
        .read_trial()
        .is_some_and(|t| t.version == crate::VERSION)
    {
        match layout.remove_trial() {
            Ok(()) => {
                tracing::info!(version = crate::VERSION, ?why, "update confirmed healthy");
            }
            Err(e) => tracing::error!(error = %e, "could not end the update trial"),
        }
    }
}

/// Call once the listener is bound: marks the version healthy after [`HEALTHY_AFTER`]
/// unless a handshake did first. A crash before then kills this task with the process.
pub fn mark_healthy_later(state: std::sync::Arc<State>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(HEALTHY_AFTER).await;
        mark_healthy(&state, Healthy::RanLongEnough);
    })
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "Test assertions fail immediately on malformed setup"
    )]
    use super::*;

    fn on_trial(home: &Path, version: &str) -> Layout {
        let layout = Layout::new(home);
        layout
            .write_trial(&Trial {
                version: version.into(),
                sha256: None,
                started_at: 0,
                attempts: 1,
            })
            .unwrap();
        layout
    }

    #[tokio::test(start_paused = true)]
    async fn a_version_that_keeps_listening_ends_its_trial_without_a_handshake() {
        let home = tempfile::tempdir().unwrap();
        let layout = on_trial(home.path(), crate::VERSION);
        let state = std::sync::Arc::new(State::new(home.path().to_path_buf(), 1));
        let timer = mark_healthy_later(state);
        tokio::time::sleep(HEALTHY_AFTER - Duration::from_secs(1)).await;
        assert!(layout.read_trial().is_some(), "ended its trial too early");
        timer.await.unwrap();
        assert_eq!(layout.read_trial(), None);
    }

    #[test]
    fn only_the_running_version_ends_a_trial() {
        let home = tempfile::tempdir().unwrap();
        let layout = on_trial(home.path(), "0.0.0-other");
        mark_healthy(
            &State::new(home.path().to_path_buf(), 1),
            Healthy::Handshake,
        );
        assert!(layout.read_trial().is_some());
    }

    #[test]
    fn chunks_must_be_in_order_within_size_and_limit() {
        assert_eq!(check_chunk(0, 10, 0, 10), Ok(()));
        assert_eq!(check_chunk(4, 10, 4, 6), Ok(()));
        assert!(check_chunk(4, 10, 0, 4).is_err(), "replayed offset");
        assert!(check_chunk(4, 10, 5, 1).is_err(), "gap");
        assert!(check_chunk(4, 10, 4, 7).is_err(), "past the size");
        let big = u64::try_from(UPDATE_CHUNK).unwrap_or(u64::MAX) * 2;
        assert!(check_chunk(0, big, 0, UPDATE_CHUNK + 1).is_err());
        assert_eq!(check_chunk(0, big, 0, UPDATE_CHUNK), Ok(()));
    }

    #[test]
    fn version_output_must_name_the_exact_version() {
        assert!(reports_version("dot-guest 1.2.0\n", "1.2.0"));
        assert!(!reports_version("dot-guest 1.2.0.1\n", "1.2.0"));
        assert!(!reports_version("dot-guest 11.2.0\n", "1.2.0"));
        assert!(!reports_version("", "1.2.0"));
    }
}
