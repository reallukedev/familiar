//! An MCP server the guest starts, speaking newline-delimited JSON-RPC on stdin/stdout.
//!
//! Servers are started directly, never through a shell, so a file written into the
//! home folder (a shell rc file, say) can't change what an approved server runs. Command
//! names are looked up in the login shell's PATH, read once per agent process.
use super::redact;
use super::rpc::{self, Fail, Incoming};
use crate::error;
use crate::proc::GroupGuard;
use dot_wire::{GuestError, GuestErrorCode};
use serde_json::Value;
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::OnceCell;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// How much of the server's stderr is kept for error messages.
const STDERR_TAIL: usize = 4096;
/// How much of that tail goes into a message.
const STDERR_SHOWN: usize = 300;
/// How long sending `notifications/cancelled` may take before the server is stopped.
const CANCEL_GRACE: Duration = Duration::from_millis(200);
/// PATH when the login shell can't be asked.
pub const DEFAULT_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
/// How long the login shell may take to print its PATH.
const LOGIN_PATH_WAIT: Duration = Duration::from_secs(10);
/// Marks the PATH line in the login shell's output, after anything its rc files print.
const PATH_MARK: &str = "__DOT_GUEST_PATH__=";

/// Environment names that make a program load extra code. Dropped from both the
/// agent's own environment and the config's before a server starts.
const DROPPED_ENV: &[&str] = &[
    "ZDOTDIR",
    "BASH_ENV",
    "ENV",
    "NODE_OPTIONS",
    "PYTHONSTARTUP",
    "PERL5OPT",
    "RUBYOPT",
    "JAVA_TOOL_OPTIONS",
    "_JAVA_OPTIONS",
];

fn dropped_env(name: &str) -> bool {
    DROPPED_ENV.contains(&name) || name.starts_with("DYLD_") || name.starts_with("LD_")
}

/// Where command names are looked up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchPath {
    /// The login shell's PATH, read once per agent process.
    LoginShell,
    /// A fixed PATH (tests, which must not depend on the user's shell setup).
    #[cfg_attr(not(test), expect(dead_code, reason = "Only tests fix the PATH"))]
    Fixed(String),
}

impl SearchPath {
    async fn get(&self) -> String {
        static LOGIN: OnceCell<String> = OnceCell::const_new();
        match self {
            SearchPath::Fixed(path) => path.clone(),
            SearchPath::LoginShell => LOGIN.get_or_init(read_login_path).await.clone(),
        }
    }
}

/// Ask `zsh -l` for its PATH. Falls back to [`DEFAULT_PATH`] if it fails or stalls.
async fn read_login_path() -> String {
    let script = format!("printf '\\n{PATH_MARK}%s\\n' \"$PATH\"");
    let out = crate::proc::Command::new("/bin/zsh", Instant::now() + LOGIN_PATH_WAIT)
        .args(["-lc", &script])
        .max_output(256 * 1024)
        .run()
        .await;
    match out {
        Ok(out) if out.success() => parse_login_path(&out.stdout_text()).unwrap_or_else(|| {
            tracing::warn!("the login shell printed no PATH; using the default");
            DEFAULT_PATH.to_owned()
        }),
        _ => {
            tracing::warn!("could not read the login shell's PATH in time; using the default");
            DEFAULT_PATH.to_owned()
        }
    }
}

/// The PATH from the marked line, ignoring whatever the shell's rc files printed.
fn parse_login_path(stdout: &str) -> Option<String> {
    let line = stdout
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(PATH_MARK))?;
    let dirs: Vec<&str> = line.split(':').filter(|dir| dir.starts_with('/')).collect();
    (!dirs.is_empty()).then(|| dirs.join(":"))
}

/// `command` as a path: itself when it has a `/`, otherwise the first executable file
/// of that name in `path`.
fn find_program(command: &str, path: &str) -> Option<PathBuf> {
    if command.contains('/') {
        return Some(PathBuf::from(command));
    }
    path.split(':')
        .filter(|dir| dir.starts_with('/'))
        .map(|dir| Path::new(dir).join(command))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

/// A running server that has finished the `initialize` handshake.
#[derive(Debug)]
pub struct Session {
    name: String,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_task: Option<JoinHandle<()>>,
    next_id: u64,
    /// The request sent and not yet answered.
    pending: Option<u64>,
    /// A write was started and not finished, so the input stream may hold half a line.
    writing: bool,
    /// The config's environment values, hidden from stderr shown in messages.
    secrets: Vec<String>,
    child: Child,
    // Dropped last: kills the server's whole process group.
    _group: GroupGuard,
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
    }
}

/// What to start.
pub struct Spawn<'a> {
    /// Display name, for messages.
    pub name: &'a str,
    /// Program.
    pub command: &'a str,
    /// Arguments.
    pub args: &'a [String],
    /// Added to the agent's environment, minus [`DROPPED_ENV`].
    pub env: &'a BTreeMap<String, String>,
    /// Working folder.
    pub cwd: &'a Path,
}

impl Session {
    /// Start the server and run the handshake before `deadline`.
    pub async fn start(
        search: &SearchPath,
        spawn: &Spawn<'_>,
        deadline: Instant,
    ) -> Result<Self, GuestError> {
        let path = match spawn.env.get("PATH") {
            Some(path) => path.clone(),
            None => search.get().await,
        };
        let program = find_program(spawn.command, &path).ok_or_else(|| {
            error::unsupported(format!(
                "`{}` is not installed on this Mac (looked in {path}). Install it, or put its full path in {}'s config.",
                spawn.command, spawn.name
            ))
        })?;
        let mut cmd = tokio::process::Command::new(&program);
        for (name, _) in std::env::vars_os() {
            if name.to_str().is_some_and(dropped_env) {
                cmd.env_remove(&name);
            }
        }
        cmd.args(spawn.args)
            .envs(spawn.env.iter().filter(|(name, _)| !dropped_env(name)))
            .env("PATH", &path)
            .current_dir(spawn.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        let program_name = program.to_string_lossy();
        let mut child = cmd
            .spawn()
            .map_err(|e| crate::proc::spawn_error(&program_name, &e))?;
        let group = GroupGuard::new(child.id());
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(error::failed(format!(
                "Could not connect to {}'s input and output.",
                spawn.name
            )));
        };
        let tail = Arc::new(Mutex::new(Vec::new()));
        let stderr_task = tokio::spawn(keep_tail(stderr, tail.clone()));
        let mut session = Self {
            name: spawn.name.to_owned(),
            stdin,
            stdout: BufReader::new(stdout),
            stderr: tail,
            stderr_task: Some(stderr_task),
            next_id: 0,
            pending: None,
            writing: false,
            secrets: spawn.env.values().cloned().collect(),
            child,
            _group: group,
        };
        let handshake = async {
            session
                .exchange("initialize", rpc::initialize_params())
                .await?;
            session
                .send(&rpc::notification("notifications/initialized"))
                .await
        };
        let outcome = tokio::time::timeout_at(deadline, handshake).await;
        match outcome {
            Ok(Ok(())) => Ok(session),
            Ok(Err(Fail::Rpc(message))) => Err(error::failed(format!(
                "{} would not start a session: {message}",
                spawn.name
            ))),
            Ok(Err(Fail::Broken(e) | Fail::Timeout(e))) => Err(e),
            Err(_) => Err(error::timeout(format!(
                "{} started but did not finish starting up in time, so it was stopped.",
                spawn.name
            ))),
        }
    }

    /// Send one request and wait for its answer until `deadline`.
    ///
    /// On a timeout the request is cancelled with `notifications/cancelled` and the
    /// session is kept ([`Fail::Timeout`]): its late answer has an old id and is skipped.
    /// It is [`Fail::Broken`] instead when the server exited or a write was cut off.
    pub async fn request(
        &mut self,
        method: &str,
        params: Value,
        deadline: Instant,
    ) -> Result<Value, Fail> {
        let outcome = tokio::time::timeout_at(deadline, self.exchange(method, params)).await;
        match outcome {
            Ok(result) => result,
            Err(_) => Err(self.after_timeout().await),
        }
    }

    async fn after_timeout(&mut self) -> Fail {
        let exited = !matches!(self.child.try_wait(), Ok(None));
        if !exited && !self.writing && self.cancel_pending().await {
            return Fail::Timeout(error::timeout(format!(
                "{} did not answer in time, so the request was cancelled.",
                self.name
            )));
        }
        Fail::Broken(error::timeout(format!(
            "{} did not answer in time, so it was stopped. The next call starts it again.",
            self.name
        )))
    }

    /// Tell the server to stop working on the pending request. False if that failed.
    async fn cancel_pending(&mut self) -> bool {
        let Some(id) = self.pending.take() else {
            return true;
        };
        let sent = tokio::time::timeout(CANCEL_GRACE, self.send(&rpc::cancelled(id))).await;
        matches!(sent, Ok(Ok(())))
    }

    /// For a call the host gave up on mid-request: cancel what's pending if the input
    /// stream is intact, then stop the server.
    pub async fn abandon(mut self) {
        if !self.writing {
            self.cancel_pending().await;
        }
    }

    async fn exchange(&mut self, method: &str, params: Value) -> Result<Value, Fail> {
        self.next_id += 1;
        let sent = self.next_id;
        self.pending = Some(sent);
        self.send(&rpc::request(sent, method, params)).await?;
        loop {
            let line = self.read_line().await?;
            // Some servers print log lines to stdout; anything that isn't JSON is skipped.
            let Ok(msg) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            match rpc::classify(msg) {
                Incoming::Response { id, outcome } if rpc::same_id(&id, sent) => {
                    self.pending = None;
                    return outcome.map_err(Fail::Rpc);
                }
                Incoming::Request { id, method } => self.send(&rpc::answer(id, &method)).await?,
                // Includes late answers to requests that timed out.
                Incoming::Response { .. } | Incoming::Other => {}
            }
        }
    }

    async fn send(&mut self, msg: &Value) -> Result<(), Fail> {
        let mut line = serde_json::to_vec(msg)
            .map_err(|e| Fail::Broken(error::failed(format!("Could not encode a message: {e}"))))?;
        line.push(b'\n');
        self.writing = true;
        let written = async {
            self.stdin.write_all(&line).await?;
            self.stdin.flush().await
        };
        let result = written.await;
        match result {
            Ok(()) => {
                self.writing = false;
                Ok(())
            }
            Err(_) => Err(Fail::Broken(self.stopped().await)),
        }
    }

    async fn read_line(&mut self) -> Result<Vec<u8>, Fail> {
        let mut line = Vec::new();
        let limit = u64::try_from(rpc::MAX_MESSAGE).unwrap_or(u64::MAX) + 1;
        let read = (&mut self.stdout)
            .take(limit)
            .read_until(b'\n', &mut line)
            .await;
        match read {
            Ok(0) | Err(_) => Err(Fail::Broken(self.stopped().await)),
            Ok(_) if line.len() > rpc::MAX_MESSAGE => Err(Fail::Broken(GuestError::new(
                GuestErrorCode::TooLarge,
                format!(
                    "{} sent a message larger than {} MB.",
                    self.name,
                    rpc::MAX_MESSAGE / (1024 * 1024)
                ),
            ))),
            Ok(_) => Ok(line),
        }
    }

    /// Explain that the server went away, with the end of what it wrote to stderr
    /// (credentials and the config's environment values hidden).
    async fn stopped(&mut self) -> GuestError {
        if let Some(mut task) = self.stderr_task.take()
            && tokio::time::timeout(Duration::from_millis(300), &mut task)
                .await
                .is_err()
        {
            task.abort();
        }
        let tail = self
            .stderr
            .lock()
            .map(|t| String::from_utf8_lossy(&t).into_owned())
            .unwrap_or_default();
        let tail = redact::redact_text(&tail, &self.secrets, STDERR_SHOWN);
        let mut message = format!("{} stopped before it answered.", self.name);
        if !tail.is_empty() {
            message.push_str(&format!(" It said: {tail}"));
        }
        error::failed(message)
    }
}

/// Keep the last [`STDERR_TAIL`] bytes of `stderr`.
async fn keep_tail(mut stderr: tokio::process::ChildStderr, tail: Arc<Mutex<Vec<u8>>>) {
    let mut buf = [0u8; 4096];
    loop {
        let n = match stderr.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let Ok(mut tail) = tail.lock() else { break };
        tail.extend_from_slice(&buf[..n]);
        if tail.len() > STDERR_TAIL {
            let cut = tail.len() - STDERR_TAIL;
            tail.drain(..cut);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_path_is_the_marked_line_even_after_rc_noise() {
        let out = format!(
            "Welcome!\n{PATH_MARK}/old\nmore noise\n\n{PATH_MARK}/opt/homebrew/bin:relative:/usr/bin\n"
        );
        assert_eq!(
            parse_login_path(&out).as_deref(),
            Some("/opt/homebrew/bin:/usr/bin")
        );
        assert_eq!(parse_login_path("no marker\n"), None);
        assert_eq!(parse_login_path(&format!("{PATH_MARK}relative\n")), None);
    }
}
