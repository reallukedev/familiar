//! The host side of the link to `dot-guest`: one authenticated TCP stream, many calls in
//! flight, matched by id. Dropping a call's future cancels it in the guest.
use dot_wire::{Envelope, GuestError, GuestInfo, Op, Reply, frame};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, WriteHalf},
    net::TcpStream,
    sync::{Mutex as AsyncMutex, oneshot},
};

/// Why a guest call failed.
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    /// The link dropped or was never up.
    #[error("the Mac's agent isn't connected")]
    Disconnected,
    /// No reply before the deadline (plus a grace period).
    #[error("the Mac didn't answer in time")]
    Timeout,
    /// The guest answered with a typed failure.
    #[error("{0}")]
    Guest(#[from] GuestError),
    /// The reply wasn't the shape we expected.
    #[error("the Mac's agent sent an unexpected reply: {0}")]
    Decode(String),
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, GuestError>>>>>;
type Writer = Arc<AsyncMutex<WriteHalf<Box<dyn Stream>>>>;

/// Any bidirectional byte stream (TCP in production, duplex pipes in tests).
pub trait Stream: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> Stream for T {}

/// An authenticated connection to one Mac.
pub struct Link {
    writer: Writer,
    pending: Pending,
    next_id: AtomicU64,
    alive: Arc<AtomicBool>,
    /// What the guest said about itself during the handshake.
    pub info: GuestInfo,
    /// The guest's version.
    pub agent_version: String,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link")
            .field("agent_version", &self.agent_version)
            .field("alive", &self.is_alive())
            .finish_non_exhaustive()
    }
}

impl Link {
    /// Dial and authenticate, within `timeout`.
    pub async fn connect(
        addr: &str,
        secret: &[u8],
        instance_id: &str,
        timeout: Duration,
    ) -> anyhow::Result<Arc<Self>> {
        let stream = tokio::time::timeout(timeout, TcpStream::connect(addr)).await??;
        stream.set_nodelay(true)?;
        tokio::time::timeout(
            timeout,
            Self::handshake(Box::new(stream), secret, instance_id),
        )
        .await?
    }

    /// Authenticate over an established stream and start the reader.
    pub async fn handshake(
        mut stream: Box<dyn Stream>,
        secret: &[u8],
        instance_id: &str,
    ) -> anyhow::Result<Arc<Self>> {
        let (hello, info) = dot_wire::handshake::host(&mut stream, secret, instance_id).await?;
        let (mut reader, writer) = tokio::io::split(stream);
        let pending: Pending = Arc::default();
        let alive = Arc::new(AtomicBool::new(true));
        let link = Arc::new(Self {
            writer: Arc::new(AsyncMutex::new(writer)),
            pending: pending.clone(),
            next_id: AtomicU64::new(1),
            alive: alive.clone(),
            info,
            agent_version: hello.agent_version,
        });
        tokio::spawn(async move {
            loop {
                match frame::read::<_, Reply>(&mut reader).await {
                    Ok(reply) => {
                        let tx = pending
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .remove(&reply.id);
                        if let Some(tx) = tx {
                            let _ = tx.send(reply.result);
                        }
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "guest link closed");
                        break;
                    }
                }
            }
            alive.store(false, Ordering::SeqCst);
            // Dropping the senders wakes every waiter with Disconnected.
            pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
        });
        Ok(link)
    }

    /// Still connected.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Run an op with a deadline and return its raw JSON result.
    pub async fn call(&self, op: Op, deadline: Duration) -> Result<Value, LinkError> {
        if !self.is_alive() {
            return Err(LinkError::Disconnected);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, tx);
        let mut guard = CancelGuard {
            link: self,
            id,
            armed: true,
        };
        let envelope = Envelope::Call {
            id,
            deadline_ms: u64::try_from(deadline.as_millis()).unwrap_or(u64::MAX),
            op,
        };
        {
            let mut w = self.writer.lock().await;
            if frame::write(&mut *w, &envelope).await.is_err() {
                self.alive.store(false, Ordering::SeqCst);
                guard.armed = false;
                self.forget(id);
                return Err(LinkError::Disconnected);
            }
        }
        // The guest enforces the deadline itself; the grace covers the trip back.
        let result = tokio::time::timeout(deadline + Duration::from_secs(3), rx).await;
        guard.armed = false;
        match result {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(e))) => Err(LinkError::Guest(e)),
            Ok(Err(_)) => Err(LinkError::Disconnected),
            Err(_) => {
                self.forget(id);
                self.cancel(id);
                Err(LinkError::Timeout)
            }
        }
    }

    /// [`Link::call`] and decode.
    pub async fn call_as<T: DeserializeOwned>(
        &self,
        op: Op,
        deadline: Duration,
    ) -> Result<T, LinkError> {
        let value = self.call(op, deadline).await?;
        serde_json::from_value(value).map_err(|e| LinkError::Decode(e.to_string()))
    }

    fn forget(&self, id: u64) {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
    }

    fn cancel(&self, id: u64) {
        let writer = self.writer.clone();
        tokio::spawn(async move {
            let mut w = writer.lock().await;
            let _ = frame::write(&mut *w, &Envelope::Cancel { id }).await;
        });
    }
}

/// Sends `Cancel` when a call's future is dropped before it finishes.
struct CancelGuard<'a> {
    link: &'a Link,
    id: u64,
    armed: bool,
}

impl Drop for CancelGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.link.forget(self.id);
            if self.link.is_alive() {
                self.link.cancel(self.id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dot_wire::GuestErrorCode;
    use serde_json::json;

    /// A tiny fake guest: answers Ping, fails Info, sleeps on Exec, records cancels.
    async fn fake_guest(stream: tokio::io::DuplexStream, cancels: Arc<Mutex<Vec<u64>>>) {
        let mut stream = stream;
        dot_wire::handshake::guest(&mut stream, b"k", "i1", "9.9.9", GuestInfo::default)
            .await
            .unwrap();
        let (mut r, w) = tokio::io::split(stream);
        let w = Arc::new(AsyncMutex::new(w));
        // Dropping the set aborts in-flight calls, so the stream really closes.
        let mut tasks = tokio::task::JoinSet::new();
        while let Ok(env) = frame::read::<_, Envelope>(&mut r).await {
            match env {
                Envelope::Cancel { id } => cancels.lock().unwrap().push(id),
                Envelope::Call { id, op, .. } => {
                    let w = w.clone();
                    tasks.spawn(async move {
                        let result = match op {
                            Op::Ping => Ok(json!({})),
                            Op::Exec(_) => {
                                tokio::time::sleep(Duration::from_secs(60)).await;
                                Ok(json!({}))
                            }
                            _ => Err(GuestError::new(GuestErrorCode::NotFound, "nope")),
                        };
                        let mut w = w.lock().await;
                        let _ = frame::write(&mut *w, &Reply { id, result }).await;
                    });
                }
            }
        }
    }

    #[tokio::test]
    async fn concurrent_calls_errors_timeouts_and_disconnects() {
        let (host, guest) = tokio::io::duplex(1 << 16);
        let cancels = Arc::new(Mutex::new(Vec::new()));
        let server = tokio::spawn(fake_guest(guest, cancels.clone()));
        let link = Link::handshake(Box::new(host), b"k", "i1").await.unwrap();
        assert_eq!(link.agent_version, "9.9.9");

        let (a, b) = tokio::join!(
            link.call(Op::Ping, Duration::from_secs(1)),
            link.call(Op::Info, Duration::from_secs(1))
        );
        assert_eq!(a.unwrap(), json!({}));
        assert!(matches!(b, Err(LinkError::Guest(e)) if e.code == GuestErrorCode::NotFound));

        // A dropped caller cancels the call in the guest.
        let slow = link.call(Op::Exec(Default::default()), Duration::from_secs(30));
        let _ = tokio::time::timeout(Duration::from_millis(50), slow).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(cancels.lock().unwrap().len(), 1);

        server.abort();
        let _ = server.await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!link.is_alive());
        assert!(matches!(
            link.call(Op::Ping, Duration::from_secs(1)).await,
            Err(LinkError::Disconnected)
        ));
    }
}
