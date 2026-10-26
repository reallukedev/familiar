//! Minimal QMP (QEMU Machine Protocol) client over a unix socket.
//!
//! One short-lived connection per operation keeps this simple and robust: QEMU accepts
//! a new QMP client whenever the previous one disconnects.
use anyhow::{Context, bail};
use serde_json::{Value, json};
use std::{
    os::fd::{AsRawFd, BorrowedFd},
    path::Path,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Interest},
    net::UnixStream,
};

/// A negotiated QMP connection.
pub struct Qmp {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

impl Qmp {
    /// Connect and leave capabilities negotiation mode.
    pub async fn connect(socket: &Path) -> anyhow::Result<Self> {
        let stream = tokio::time::timeout(Duration::from_secs(3), UnixStream::connect(socket))
            .await
            .context("QMP connect timed out")??;
        let (r, w) = stream.into_split();
        let mut qmp = Self {
            reader: BufReader::new(r),
            writer: w,
        };
        // QMP serves one client at a time; a second one connects but gets no greeting.
        let greeting = qmp.read_within(Duration::from_secs(5)).await?;
        if greeting.get("QMP").is_none() {
            bail!("not a QMP socket");
        }
        qmp.execute("qmp_capabilities", None).await?;
        Ok(qmp)
    }

    async fn read(&mut self) -> anyhow::Result<Value> {
        self.read_within(Duration::from_secs(30)).await
    }

    async fn read_within(&mut self, limit: Duration) -> anyhow::Result<Value> {
        let mut line = String::new();
        let n = tokio::time::timeout(limit, self.reader.read_line(&mut line))
            .await
            .context("QMP read timed out")??;
        if n == 0 {
            bail!("QMP closed");
        }
        Ok(serde_json::from_str(&line)?)
    }

    /// Run a command and return its `return` value. Async events are skipped.
    pub async fn execute(
        &mut self,
        command: &str,
        arguments: Option<Value>,
    ) -> anyhow::Result<Value> {
        let line = Self::line(command, arguments)?;
        self.writer.write_all(&line).await?;
        self.reply(command).await
    }

    fn line(command: &str, arguments: Option<Value>) -> anyhow::Result<Vec<u8>> {
        let mut msg = json!({ "execute": command });
        if let Some(args) = arguments {
            msg["arguments"] = args;
        }
        let mut line = serde_json::to_vec(&msg)?;
        line.push(b'\n');
        Ok(line)
    }

    async fn reply(&mut self, command: &str) -> anyhow::Result<Value> {
        loop {
            let v = self.read().await?;
            if v.get("event").is_some() {
                continue;
            }
            if let Some(err) = v.get("error") {
                bail!(
                    "QEMU refused {command}: {}",
                    err.get("desc")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error")
                );
            }
            if let Some(ret) = v.get("return") {
                return Ok(ret.clone());
            }
        }
    }

    /// Hand QEMU an open file descriptor (`getfd` with SCM_RIGHTS) under `name`, for
    /// `fd:<name>` migration URIs. QEMU keeps its own copy; the caller closes theirs.
    pub async fn send_fd(&mut self, name: &str, fd: BorrowedFd<'_>) -> anyhow::Result<()> {
        use nix::sys::socket::{ControlMessage, MsgFlags, sendmsg};
        let line = Self::line("getfd", Some(json!({ "fdname": name })))?;
        let stream: &UnixStream = self.writer.as_ref();
        let fds = [fd.as_raw_fd()];
        loop {
            stream.writable().await?;
            let sent = stream.try_io(Interest::WRITABLE, || {
                sendmsg::<()>(
                    stream.as_raw_fd(),
                    &[std::io::IoSlice::new(&line)],
                    &[ControlMessage::ScmRights(&fds)],
                    MsgFlags::empty(),
                    None,
                )
                .map_err(std::io::Error::from)
            });
            match sent {
                Ok(n) if n == line.len() => break,
                Ok(_) => bail!("QMP: a short write while passing a file descriptor"),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.into()),
            }
        }
        self.reply("getfd").await.map(|_| ())
    }

    /// Run a human monitor command (savevm, loadvm, delvm). HMP reports failures as text.
    pub async fn hmp(&mut self, command_line: &str) -> anyhow::Result<String> {
        let out = self
            .execute(
                "human-monitor-command",
                Some(json!({ "command-line": command_line })),
            )
            .await?;
        let text = out.as_str().unwrap_or_default().trim().to_owned();
        let lower = text.to_ascii_lowercase();
        if lower.contains("error") || lower.contains("could not") || lower.contains("failed") {
            bail!("QEMU: {text}");
        }
        Ok(text)
    }

    /// `running`, `paused`, `inmigrate`, `postmigrate`, `shutdown`, ...
    pub async fn status(&mut self) -> anyhow::Result<String> {
        let v = self.execute("query-status", None).await?;
        Ok(v.get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned())
    }
}

/// Connect, run one command, disconnect.
pub async fn once(socket: &Path, command: &str, arguments: Option<Value>) -> anyhow::Result<Value> {
    Qmp::connect(socket)
        .await?
        .execute(command, arguments)
        .await
}

#[cfg(test)]
pub mod fake {
    //! A scripted QMP server for tests.
    use serde_json::{Value, json};
    use std::{path::Path, sync::Arc};
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::UnixListener,
        sync::Mutex,
    };

    /// Serves QMP on `socket`, recording commands; `status` is what query-status says.
    pub async fn serve(socket: &Path, status: Arc<Mutex<String>>, log: Arc<Mutex<Vec<String>>>) {
        let listener = UnixListener::bind(socket).unwrap();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let status = status.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let (r, mut w) = stream.into_split();
                    let mut r = BufReader::new(r);
                    let _ = w.write_all(b"{\"QMP\":{\"version\":{}}}\n").await;
                    let mut line = String::new();
                    while r.read_line(&mut line).await.unwrap_or(0) > 0 {
                        let v: Value = serde_json::from_str(&line).unwrap_or_default();
                        line.clear();
                        let cmd = v["execute"].as_str().unwrap_or("").to_owned();
                        log.lock().await.push(cmd.clone());
                        let reply = match cmd.as_str() {
                            "query-status" => json!({"return": {"status": *status.lock().await}}),
                            "stop" => {
                                *status.lock().await = "paused".into();
                                json!({"return": {}})
                            }
                            "cont" => {
                                *status.lock().await = "running".into();
                                json!({"return": {}})
                            }
                            "bogus" => {
                                json!({"error": {"class": "GenericError", "desc": "no such command"}})
                            }
                            _ => json!({"return": {}}),
                        };
                        // An async event first, to prove it's skipped.
                        let _ = w.write_all(b"{\"event\":\"RTC_CHANGE\"}\n").await;
                        let _ = w.write_all(format!("{reply}\n").as_bytes()).await;
                    }
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn negotiates_skips_events_and_surfaces_errors() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("qmp.sock");
        let status = Arc::new(Mutex::new("running".to_owned()));
        let log = Arc::new(Mutex::new(Vec::new()));
        fake::serve(&sock, status.clone(), log.clone()).await;
        let mut q = Qmp::connect(&sock).await.unwrap();
        assert_eq!(q.status().await.unwrap(), "running");
        q.execute("stop", None).await.unwrap();
        assert_eq!(q.status().await.unwrap(), "paused");
        let err = q.execute("bogus", None).await.unwrap_err().to_string();
        assert!(err.contains("no such command"));
        assert_eq!(
            log.lock().await.first().map(String::as_str),
            Some("qmp_capabilities")
        );
    }
}
