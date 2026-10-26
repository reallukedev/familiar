//! Child processes with a deadline, an output cap and process-group cleanup.
//!
//! Every child is the leader of its own process group. If the deadline passes, or the
//! future running it is dropped (the call was cancelled or the connection went away),
//! the whole group is killed, so `zsh -lc 'sleep 100 & wait'` can't leave `sleep` behind.
use crate::error::{self, OpResult};
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// Output cap for internal tools when the caller doesn't set one.
pub const DEFAULT_TOOL_OUTPUT: usize = 16 * 1024 * 1024;
/// How long to keep reading after the child exits, for grandchildren that still hold
/// the pipes open (e.g. `server &`).
const DRAIN_GRACE: Duration = Duration::from_millis(300);

/// A command to run.
#[derive(Debug)]
pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    env: Vec<(OsString, OsString)>,
    stdin: Option<Vec<u8>>,
    max_output: usize,
    deadline: Instant,
}

/// What a finished command produced.
#[derive(Debug, Default)]
pub struct Output {
    /// Exit status; `None` when a signal ended it.
    pub code: Option<i32>,
    /// Captured stdout, up to the cap.
    pub stdout: Vec<u8>,
    /// Captured stderr, up to the cap.
    pub stderr: Vec<u8>,
    /// Some output was dropped because of the cap.
    pub truncated: bool,
    /// The deadline killed it.
    pub timed_out: bool,
    /// Wall time.
    pub duration: Duration,
}

impl Output {
    /// Exited with status 0.
    pub fn success(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }
    /// Stdout as lossy UTF-8.
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
    /// Stderr as lossy UTF-8, trimmed.
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_owned()
    }
}

impl Command {
    /// Run `program` with a hard deadline.
    pub fn new(program: impl AsRef<OsStr>, deadline: Instant) -> Self {
        Self {
            program: program.as_ref().to_owned(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            stdin: None,
            max_output: DEFAULT_TOOL_OUTPUT,
            deadline,
        }
    }
    /// Add one argument.
    #[must_use]
    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }
    /// Add arguments.
    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }
    /// Working directory.
    #[must_use]
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }
    /// Set an environment variable.
    #[must_use]
    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.env
            .push((key.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }
    /// Bytes written to stdin, which is then closed. Without this stdin is `/dev/null`.
    #[must_use]
    pub fn stdin(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(bytes.into());
        self
    }
    /// Combined cap for stdout and stderr.
    #[must_use]
    pub fn max_output(mut self, bytes: usize) -> Self {
        self.max_output = bytes;
        self
    }

    /// Run to completion or until the deadline.
    pub async fn run(self) -> io::Result<Output> {
        let started = Instant::now();
        let mut cmd = tokio::process::Command::new(&self.program);
        cmd.args(&self.args)
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(if self.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
        }
        let mut child = cmd.spawn()?;
        let mut group = GroupGuard::new(child.id());

        let budget = Arc::new(AtomicUsize::new(self.max_output));
        let truncated = Arc::new(AtomicBool::new(false));
        let stdout_buf = Arc::new(Mutex::new(Vec::new()));
        let stderr_buf = Arc::new(Mutex::new(Vec::new()));
        let mut readers = Vec::with_capacity(3);
        if let Some(out) = child.stdout.take() {
            readers.push(AbortOnDrop(tokio::spawn(read_capped(
                out,
                stdout_buf.clone(),
                budget.clone(),
                truncated.clone(),
            ))));
        }
        if let Some(err) = child.stderr.take() {
            readers.push(AbortOnDrop(tokio::spawn(read_capped(
                err,
                stderr_buf.clone(),
                budget.clone(),
                truncated.clone(),
            ))));
        }
        let _stdin_writer = match (child.stdin.take(), self.stdin) {
            (Some(mut pipe), Some(bytes)) => Some(AbortOnDrop(tokio::spawn(async move {
                // A child that exits without reading stdin is not an error.
                let _ = pipe.write_all(&bytes).await;
                let _ = pipe.shutdown().await;
            }))),
            _ => None,
        };

        let status = tokio::select! {
            status = child.wait() => Some(status?),
            () = tokio::time::sleep_until(self.deadline) => None,
        };
        let timed_out = status.is_none();
        let status = match status {
            Some(status) => status,
            None => {
                group.kill();
                let _ = child.start_kill();
                child.wait().await?
            }
        };
        group.disarm();

        let grace_end = Instant::now() + DRAIN_GRACE;
        for reader in &mut readers {
            if tokio::time::timeout_at(grace_end, &mut reader.0)
                .await
                .is_err()
            {
                reader.0.abort();
            }
        }
        drop(readers);

        Ok(Output {
            code: status.code(),
            stdout: take(&stdout_buf),
            stderr: take(&stderr_buf),
            truncated: truncated.load(Ordering::Relaxed),
            timed_out,
            duration: started.elapsed(),
        })
    }
}

fn take(buf: &Mutex<Vec<u8>>) -> Vec<u8> {
    std::mem::take(
        &mut *buf
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// Read everything, keeping at most what the shared budget allows. Keeps draining past
/// the cap so the child never blocks on a full pipe.
async fn read_capped<R: AsyncRead + Unpin>(
    mut reader: R,
    sink: Arc<Mutex<Vec<u8>>>,
    budget: Arc<AtomicUsize>,
    truncated: Arc<AtomicBool>,
) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let granted = reserve(&budget, n);
        if granted > 0 {
            sink.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(&buf[..granted]);
        }
        if granted < n {
            truncated.store(true, Ordering::Relaxed);
        }
    }
}

/// Take up to `want` bytes from the budget; returns how many were granted.
fn reserve(budget: &AtomicUsize, want: usize) -> usize {
    let mut left = budget.load(Ordering::Relaxed);
    loop {
        let granted = left.min(want);
        match budget.compare_exchange_weak(
            left,
            left - granted,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return granted,
            Err(now) => left = now,
        }
    }
}

struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Kills the child's process group unless disarmed.
#[derive(Debug)]
pub(crate) struct GroupGuard {
    pgid: Option<rustix::process::Pid>,
}

impl GroupGuard {
    /// Guard the group led by `pid` (a child started with `process_group(0)`).
    pub(crate) fn new(pid: Option<u32>) -> Self {
        let pgid = pid
            .and_then(|p| i32::try_from(p).ok())
            .and_then(rustix::process::Pid::from_raw);
        Self { pgid }
    }
    fn kill(&self) {
        if let Some(pgid) = self.pgid {
            let _ = rustix::process::kill_process_group(pgid, rustix::process::Signal::KILL);
        }
    }
    fn disarm(&mut self) {
        self.pgid = None;
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Run a system tool for an operation, mapping launch failures and the deadline to
/// [`dot_wire::GuestError`]s. A nonzero exit is returned as-is for the caller to judge.
pub async fn tool(cmd: Command) -> OpResult<Output> {
    let name = cmd.program.to_string_lossy().into_owned();
    let out = cmd.run().await.map_err(|e| spawn_error(&name, &e))?;
    if out.timed_out {
        return Err(error::timeout(format!(
            "`{}` did not finish before the deadline and was stopped.",
            short_name(&name)
        )));
    }
    Ok(out)
}

/// Like [`tool`], but a nonzero exit becomes [`dot_wire::GuestErrorCode::Failed`] with stderr.
pub async fn tool_ok(cmd: Command) -> OpResult<Output> {
    let name = cmd.program.to_string_lossy().into_owned();
    let out = tool(cmd).await?;
    if !out.success() {
        let stderr = out.stderr_text();
        let detail = if stderr.is_empty() {
            format!("exit status {:?}", out.code)
        } else {
            stderr
        };
        return Err(error::failed(format!(
            "`{}` failed: {detail}",
            short_name(&name)
        )));
    }
    Ok(out)
}

fn short_name(program: &str) -> &str {
    program.rsplit('/').next().unwrap_or(program)
}

/// Explain why a program could not be started.
pub fn spawn_error(program: &str, err: &io::Error) -> dot_wire::GuestError {
    match err.kind() {
        io::ErrorKind::NotFound => error::unsupported(format!(
            "`{}` is not installed on this Mac.",
            short_name(program)
        )),
        io::ErrorKind::PermissionDenied => error::permission(
            format!(
                "macOS would not let the agent start `{}`.",
                short_name(program)
            ),
            "Check that the program is executable and not blocked by Gatekeeper.",
        ),
        _ => error::failed(format!("Could not start `{}`: {err}", short_name(program))),
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "Test assertions fail immediately on malformed setup"
    )]
    use super::*;

    fn soon(ms: u64) -> Instant {
        Instant::now() + Duration::from_millis(ms)
    }

    #[tokio::test]
    async fn combined_output_cap_truncates_and_still_drains_the_pipe() {
        // 1 MB of output against a 1000-byte cap: the child must not block on a full pipe.
        let out = Command::new("/bin/zsh", soon(10_000))
            .args(["-c", "head -c 1000000 /dev/zero | tr '\\0' a; echo err >&2"])
            .max_output(1000)
            .run()
            .await
            .unwrap();
        assert!(out.success());
        assert!(out.truncated);
        assert_eq!(out.stdout.len() + out.stderr.len(), 1000);
    }

    #[tokio::test]
    async fn background_grandchild_holding_stdout_does_not_hang_a_finished_command() {
        let started = Instant::now();
        let out = Command::new("/bin/zsh", soon(10_000))
            .args(["-c", "echo hi; sleep 3 &"])
            .run()
            .await
            .unwrap();
        assert!(out.success());
        assert_eq!(out.stdout_text(), "hi\n");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
