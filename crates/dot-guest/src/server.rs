//! The daemon: accept the host, authenticate it, run its calls concurrently.
//!
//! One authenticated connection is served at a time; a newer one replaces it (the old
//! one is usually a half-open socket left by sleep or a network blip). Replies go
//! through a single writer task so frames never interleave.
use crate::config::Config;
use crate::error;
use crate::ops::{self, CAPABILITIES, Ctx, State, UPDATE_CAPABILITY, info};
use dot_wire::frame::{self, FrameError};
use dot_wire::{Envelope, GuestError, GuestErrorCode, Op, Reply};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc, oneshot, watch};
use tokio::task::{AbortHandle, JoinSet};
use tokio::time::Instant;

/// Calls run at once across the daemon; more are answered with `Busy`.
pub const MAX_CONCURRENT_CALLS: usize = 16;
/// Handshakes in progress at once; more connections are dropped.
const MAX_PENDING_HANDSHAKES: usize = 8;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Slows down guessing: a failed handshake holds its slot this long.
const FAILED_HANDSHAKE_DELAY: Duration = Duration::from_secs(2);
/// In-flight calls get this long to finish on shutdown.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
const MIN_DEADLINE: Duration = Duration::from_millis(100);
const MAX_DEADLINE: Duration = Duration::from_secs(6 * 3600);
/// Ops stop themselves at the deadline; this backstop catches anything that doesn't.
const DEADLINE_BACKSTOP: Duration = Duration::from_secs(2);

/// Everything the accept loop shares with connections.
struct Server {
    config: Config,
    state: Arc<State>,
    calls: Arc<Semaphore>,
    handshakes: Arc<Semaphore>,
    active: Mutex<Option<(u64, oneshot::Sender<()>)>>,
    next_conn: AtomicU64,
}

/// Serve `listener` until `shutdown` resolves, then give in-flight calls
/// [`SHUTDOWN_GRACE`] to finish.
pub async fn serve(
    listener: TcpListener,
    config: Config,
    state: Arc<State>,
    shutdown: impl Future<Output = ()>,
) {
    let server = Arc::new(Server {
        config,
        state,
        calls: Arc::new(Semaphore::new(MAX_CONCURRENT_CALLS)),
        handshakes: Arc::new(Semaphore::new(MAX_PENDING_HANDSHAKES)),
        active: Mutex::new(None),
        next_conn: AtomicU64::new(1),
    });
    let (stop_tx, stop_rx) = watch::channel(false);
    let mut conns = JoinSet::new();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    conns.spawn(connection(server.clone(), stream, peer, stop_rx.clone()));
                }
                Err(e) => {
                    // Out of file descriptors or a transient network error: keep serving.
                    tracing::warn!(error = %e, "accept failed");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            },
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
        }
    }
    drop(listener);
    tracing::info!("shutting down; letting in-flight calls finish");
    let _ = stop_tx.send(true);
    let drained = tokio::time::timeout(SHUTDOWN_GRACE + Duration::from_secs(1), async {
        while conns.join_next().await.is_some() {}
    })
    .await;
    if drained.is_err() {
        conns.abort_all();
    }
}

fn tune(stream: &TcpStream) {
    let _ = stream.set_nodelay(true);
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(30))
        .with_interval(Duration::from_secs(10));
    let _ = socket2::SockRef::from(stream).set_tcp_keepalive(&keepalive);
}

async fn connection(
    server: Arc<Server>,
    mut stream: TcpStream,
    peer: SocketAddr,
    stop: watch::Receiver<bool>,
) {
    tune(&stream);
    let Ok(permit) = server.handshakes.clone().try_acquire_owned() else {
        tracing::warn!(%peer, "too many handshakes in progress; dropping connection");
        return;
    };
    let info = info::cached(&server.state, Duration::from_secs(5)).await;
    let config = &server.config;
    let outcome = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        dot_wire::handshake::guest(
            &mut stream,
            &config.secret,
            &config.instance_id,
            crate::VERSION,
            move || info,
        ),
    )
    .await;
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::warn!(%peer, error = %e, "handshake failed");
            tokio::time::sleep(FAILED_HANDSHAKE_DELAY).await;
            return;
        }
        Err(_) => {
            tracing::warn!(%peer, "handshake timed out");
            return;
        }
    }
    drop(permit);
    ops::update::mark_healthy(&server.state, ops::update::Healthy::Handshake);

    let id = server.next_conn.fetch_add(1, Ordering::Relaxed);
    let (kick_tx, kick_rx) = oneshot::channel();
    let previous = server
        .active
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .replace((id, kick_tx));
    if let Some((old, kick)) = previous {
        tracing::info!(old, new = id, "a new host connection replaces the old one");
        let _ = kick.send(());
    }
    tracing::info!(%peer, conn = id, "host connected");
    Conn::new(server.clone(), stream).run(kick_rx, stop).await;
    tracing::info!(%peer, conn = id, "host disconnected");
    let mut active = server
        .active
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if active.as_ref().is_some_and(|(current, _)| *current == id) {
        *active = None;
    }
}

type Calls = Arc<Mutex<HashMap<u64, AbortHandle>>>;

/// One authenticated connection.
struct Conn {
    server: Arc<Server>,
    reader: OwnedReadHalf,
    tx: mpsc::Sender<Reply>,
    writer: tokio::task::JoinHandle<()>,
    calls: Calls,
}

fn lock(calls: &Calls) -> std::sync::MutexGuard<'_, HashMap<u64, AbortHandle>> {
    calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Conn {
    fn new(server: Arc<Server>, stream: TcpStream) -> Self {
        let (reader, writer) = stream.into_split();
        let (tx, rx) = mpsc::channel(64);
        Self {
            server,
            reader,
            tx,
            writer: tokio::spawn(write_replies(writer, rx)),
            calls: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn run(mut self, mut kick: oneshot::Receiver<()>, mut stop: watch::Receiver<bool>) {
        let graceful = loop {
            tokio::select! {
                biased;
                _ = &mut kick => break false,
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        break true;
                    }
                }
                frame = frame::read::<_, Value>(&mut self.reader) => match frame {
                    Ok(value) => self.envelope(value).await,
                    Err(FrameError::Closed) => break false,
                    Err(e) => {
                        tracing::warn!(error = %e, "connection error");
                        break false;
                    }
                },
            }
        };
        if graceful {
            let until = Instant::now() + SHUTDOWN_GRACE;
            while !lock(&self.calls).is_empty() && Instant::now() < until {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        // Nobody is left to answer: stop the work and its child processes.
        for (_, call) in lock(&self.calls).drain() {
            call.abort();
        }
        drop(self.tx);
        let mut writer = self.writer;
        if tokio::time::timeout(Duration::from_secs(1), &mut writer)
            .await
            .is_err()
        {
            writer.abort();
        }
    }

    async fn reply(&self, id: u64, result: Result<Value, GuestError>) {
        let _ = self.tx.send(Reply { id, result }).await;
    }

    async fn envelope(&mut self, value: Value) {
        let id = value.get("id").and_then(Value::as_u64);
        let is_call = value.get("kind").and_then(Value::as_str) == Some("call");
        let op_name = value
            .pointer("/op/op")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let envelope = match serde_json::from_value::<Envelope>(value) {
            Ok(e) => e,
            Err(e) => {
                if let (Some(id), true) = (id, is_call) {
                    let err = match op_name {
                        Some(name)
                            if !CAPABILITIES.contains(&name.as_str())
                                && name != UPDATE_CAPABILITY =>
                        {
                            error::unsupported(format!(
                                "This agent (dot-guest {}) does not know the `{name}` operation. Update dot-guest in the Mac.",
                                crate::VERSION
                            ))
                        }
                        _ => error::invalid(format!("The request could not be read: {e}")),
                    };
                    self.reply(id, Err(err)).await;
                } else {
                    tracing::warn!(error = %e, "ignored a malformed envelope");
                }
                return;
            }
        };
        match envelope {
            Envelope::Call {
                id,
                deadline_ms,
                op,
            } => self.call(id, deadline_ms, op).await,
            Envelope::Cancel { id } => {
                let call = lock(&self.calls).remove(&id);
                if let Some(call) = call {
                    call.abort();
                    tracing::debug!(id, "call cancelled");
                    self.reply(
                        id,
                        Err(GuestError::new(
                            GuestErrorCode::Cancelled,
                            "The call was cancelled.",
                        )),
                    )
                    .await;
                }
            }
        }
    }

    async fn call(&mut self, id: u64, deadline_ms: u64, op: Op) {
        // Heartbeats must never queue behind work or be refused as busy.
        if matches!(op, Op::Ping) {
            self.reply(id, Ok(json!({}))).await;
            return;
        }
        let budget = Duration::from_millis(deadline_ms).clamp(MIN_DEADLINE, MAX_DEADLINE);
        let deadline = Instant::now() + budget;
        if lock(&self.calls).contains_key(&id) {
            self.reply(
                id,
                Err(error::invalid(format!("Call id {id} is already running."))),
            )
            .await;
            return;
        }
        let Ok(permit) = self.server.calls.clone().try_acquire_owned() else {
            self.reply(
                id,
                Err(GuestError::new(
                    GuestErrorCode::Busy,
                    format!("The Mac is already running {MAX_CONCURRENT_CALLS} calls. Try again shortly."),
                )),
            )
            .await;
            return;
        };
        let ctx = Ctx {
            deadline,
            state: self.server.state.clone(),
        };
        let tx = self.tx.clone();
        let registry = self.calls.clone();
        let name = op.name();
        // The registry lock is held until the handle is stored, so even an instant
        // call finds itself registered when it finishes. Only this task adds ids, so
        // the duplicate check above still holds.
        let mut calls = lock(&self.calls);
        let task = tokio::spawn(async move {
            let _permit = permit;
            let started = Instant::now();
            let result = match tokio::time::timeout_at(
                deadline + DEADLINE_BACKSTOP,
                ops::dispatch(op, &ctx),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Err(error::timeout(format!(
                    "The {name} call did not finish within its {} ms deadline.",
                    budget.as_millis()
                ))),
            };
            let outcome = result
                .as_ref()
                .map_or_else(|e| format!("{:?}", e.code), |_| "ok".to_owned());
            tracing::info!(
                id,
                op = name,
                ms = started.elapsed().as_millis() as u64,
                outcome,
                "call"
            );
            let still_wanted = lock(&registry).remove(&id).is_some();
            if still_wanted {
                let _ = tx.send(Reply { id, result }).await;
            }
        });
        calls.insert(id, task.abort_handle());
    }
}

async fn write_replies(mut writer: OwnedWriteHalf, mut rx: mpsc::Receiver<Reply>) {
    while let Some(reply) = rx.recv().await {
        match frame::write(&mut writer, &reply).await {
            Ok(()) => {}
            Err(FrameError::TooLarge(size)) => {
                let err = GuestError::new(
                    GuestErrorCode::TooLarge,
                    format!(
                        "The result is {} MiB, over the {} MiB limit. Ask for less (a region, a smaller max_width, an offset and length).",
                        size / (1024 * 1024),
                        dot_wire::MAX_FRAME / (1024 * 1024)
                    ),
                );
                let fallback = Reply {
                    id: reply.id,
                    result: Err(err),
                };
                if frame::write(&mut writer, &fallback).await.is_err() {
                    break;
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "could not send a reply");
                break;
            }
        }
    }
}
