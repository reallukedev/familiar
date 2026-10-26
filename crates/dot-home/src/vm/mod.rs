//! Every Mac and the supervisor task that keeps it in the state its owner wants.
//!
//! Each machine has one supervisor task that owns all transitions, so two requests can
//! never race a VM into a half-started state. Callers send commands and watch status.
pub mod configdisk;
pub mod memfile;
pub mod qemu;
pub mod qmp;
pub mod secrets;

use crate::{
    config::Config,
    crypto::MasterKey,
    db::{Db, now},
    events::Events,
    guest::Link,
    keys::{Keyring, MacKey},
};
use dot_wire::{GuestInfo, Op};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc, oneshot, watch};

/// Fresh boot or restore from a hibernation file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    /// Cold boot.
    Fresh,
    /// Restore RAM from `hibernate.state`.
    Resume,
}

/// What a running guest is doing, sampled to spot one that froze.
#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    /// Disk reads, writes and flushes so far, across all disks.
    pub io_ops: u64,
    /// CPU time QEMU has used so far.
    pub cpu_seconds: f64,
    /// Each vCPU's instruction pointer.
    pub rips: Vec<u64>,
}

/// Where the macOS kernel lives; firmware (OpenCore, OVMF) runs below 4 GB.
const KERNEL_SPACE: u64 = 0xffff_8000_0000_0000;

/// Spots a guest frozen the way macOS freezes when its restart fails: every vCPU in the
/// kernel, a core busy spinning, and not one disk operation.
#[derive(Debug, Default)]
struct FreezeWatch {
    since: Option<(std::time::Instant, Activity)>,
}

impl FreezeWatch {
    /// Feed one sample; true once the guest has looked frozen for `window`.
    fn observe(
        &mut self,
        now: std::time::Instant,
        sample: Option<Activity>,
        window: Duration,
    ) -> bool {
        let Some(sample) =
            sample.filter(|a| !a.rips.is_empty() && a.rips.iter().all(|r| *r >= KERNEL_SPACE))
        else {
            self.since = None;
            return false;
        };
        let Some((start, first)) = &self.since else {
            self.since = Some((now, sample));
            return false;
        };
        let elapsed = now.duration_since(*start);
        if sample.io_ops != first.io_ops {
            self.since = Some((now, sample));
            return false;
        }
        if elapsed < window {
            return false;
        }
        let busy = (sample.cpu_seconds - first.cpu_seconds) / elapsed.as_secs_f64();
        if busy >= 0.9 {
            return true;
        }
        // Idle in the kernel isn't this freeze; start over.
        self.since = Some((now, sample));
        false
    }
}

/// Shown to the owner when a Mac needs its key and they haven't signed in since the
/// server started.
pub const LOCKED: &str = "This Mac is locked. Sign in again to unlock it.";
/// The status line of a locked Mac.
pub const LOCKED_DETAIL: &str = "Locked. Sign in to unlock.";
const HIBERNATED_DETAIL: &str = "Hibernated. It wakes when your assistant needs it.";
const ENCRYPTING: &str = "This Mac is being encrypted. It can start when that's done.";

/// A Mac needs its owner's key, which isn't unlocked.
#[derive(Debug, thiserror::Error)]
#[error("{LOCKED}")]
pub struct LockedError;

/// How a Mac's files are encrypted, as the hypervisor needs to know.
#[derive(Debug, Clone)]
pub enum Crypt {
    /// Plain files: Macs from before encryption, and image builders.
    Plain,
    /// Encrypted, with the owner's key unlocked.
    Key(MacKey),
    /// Encrypted, and the owner hasn't signed in since the server started.
    Locked,
}

impl Crypt {
    /// The key, `None` for plain files, or [`LockedError`].
    pub fn key(&self) -> anyhow::Result<Option<&MacKey>> {
        match self {
            Crypt::Plain => Ok(None),
            Crypt::Key(k) => Ok(Some(k)),
            Crypt::Locked => Err(LockedError.into()),
        }
    }
    /// The files are encrypted (whether or not the key is at hand).
    pub fn is_encrypted(&self) -> bool {
        !matches!(self, Crypt::Plain)
    }
}

/// What the supervisor needs from a hypervisor. [`qemu::Qemu`] in production.
pub trait Hypervisor: Send + Sync + 'static {
    /// Create disks for a new instance (encrypted when `crypt` has a key).
    fn provision(
        &self,
        files: &qemu::Files,
        disk_gb: u32,
        from_base: bool,
        crypt: &Crypt,
    ) -> impl Future<Output = anyhow::Result<()>> + Send;
    /// Start QEMU (returns once it's running in the background).
    fn launch(
        &self,
        files: &qemu::Files,
        spec: &qemu::Spec,
        mode: LaunchMode,
        crypt: &Crypt,
    ) -> impl Future<Output = anyhow::Result<()>> + Send;
    /// QEMU's run state, or `None` when no QEMU is running.
    fn status(&self, files: &qemu::Files) -> impl Future<Output = Option<String>> + Send;
    /// A sample of what the guest is doing (`None` when it can't be read).
    fn activity(&self, files: &qemu::Files) -> impl Future<Output = Option<Activity>> + Send;
    /// ACPI power button.
    fn powerdown(&self, files: &qemu::Files) -> impl Future<Output = anyhow::Result<()>> + Send;
    /// Stop QEMU immediately.
    fn kill(&self, files: &qemu::Files) -> impl Future<Output = anyhow::Result<()>> + Send;
    /// Hardware reset.
    fn reset(&self, files: &qemu::Files) -> impl Future<Output = anyhow::Result<()>> + Send;
    /// Pause vCPUs.
    fn pause(&self, files: &qemu::Files) -> impl Future<Output = anyhow::Result<()>> + Send;
    /// Resume vCPUs.
    fn resume(&self, files: &qemu::Files) -> impl Future<Output = anyhow::Result<()>> + Send;
    /// Save RAM to disk and stop QEMU.
    fn hibernate(
        &self,
        files: &qemu::Files,
        crypt: &Crypt,
    ) -> impl Future<Output = anyhow::Result<()>> + Send;
    /// Take a snapshot.
    fn snapshot(
        &self,
        files: &qemu::Files,
        tag: &str,
        live: bool,
        crypt: &Crypt,
    ) -> impl Future<Output = anyhow::Result<()>> + Send;
    /// Restore a snapshot. A running Mac may be restarted from it (`spec`).
    fn restore(
        &self,
        files: &qemu::Files,
        tag: &str,
        running: bool,
        crypt: &Crypt,
        spec: &qemu::Spec,
    ) -> impl Future<Output = anyhow::Result<()>> + Send;
    /// Delete a snapshot.
    fn delete_snapshot(
        &self,
        files: &qemu::Files,
        tag: &str,
        running: bool,
        crypt: &Crypt,
    ) -> impl Future<Output = anyhow::Result<()>> + Send;
    /// Encrypt a stopped Mac's plain disks in place, reporting progress (0 to 1).
    fn encrypt(
        &self,
        files: &qemu::Files,
        key: &MacKey,
        progress: &(dyn Fn(f64) + Send + Sync),
    ) -> impl Future<Output = anyhow::Result<()>> + Send;
    /// Bytes the instance's disk uses on the host.
    fn disk_bytes(&self, files: &qemu::Files) -> impl Future<Output = Option<u64>> + Send;
    /// Memory saved by live snapshots inside the disk, in bytes (`None` when unknown).
    fn snapshot_state_bytes(&self, files: &qemu::Files)
    -> impl Future<Output = Option<u64>> + Send;
    /// Memory saved by each live snapshot, by tag (`None` when unknown). Disk-only
    /// snapshots share blocks with the disk and are missing or 0.
    fn snapshot_sizes(
        &self,
        files: &qemu::Files,
    ) -> impl Future<Output = Option<HashMap<String, u64>>> + Send;
}

/// Lifecycle states (see docs/home/ARCHITECTURE.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Off.
    Stopped,
    /// QEMU is being launched.
    Starting,
    /// QEMU runs; waiting for macOS and the agent.
    Booting,
    /// Agent connected and answering.
    Ready,
    /// QEMU runs but the agent stopped answering.
    Degraded,
    /// vCPUs paused; RAM still held.
    Sleeping,
    /// RAM saved to disk; QEMU not running.
    Hibernated,
    /// Shutting down.
    Stopping,
    /// Gave up after repeated failures; needs the owner.
    Error,
    /// Its disks are being converted to encrypted ones.
    Encrypting,
}

impl State {
    /// Holds host RAM.
    pub fn uses_memory(self) -> bool {
        matches!(
            self,
            State::Starting
                | State::Booting
                | State::Ready
                | State::Degraded
                | State::Sleeping
                | State::Stopping
        )
    }
}

/// State plus a human sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// Where it is.
    pub state: State,
    /// Why, for the owner.
    pub detail: Option<String>,
}

/// Instance settings the supervisor needs (refreshed from the database).
#[derive(Debug, Clone)]
pub struct Settings {
    /// Owner.
    pub user_id: String,
    /// `qemu` or `remote`.
    pub driver: String,
    /// vCPUs.
    pub cpus: u32,
    /// RAM.
    pub memory_mb: u32,
    /// Disk.
    pub disk_gb: u32,
    /// Forwarded port (qemu).
    pub port: Option<u16>,
    /// `host:port` (remote).
    pub remote_host: Option<String>,
    /// Guest secret (opened).
    pub secret: Vec<u8>,
    /// Owner wants it on.
    pub desired_running: bool,
    /// Minutes idle before sleeping (0 = never).
    pub sleep_after_minutes: i64,
    /// Minutes idle before hibernating (0 = never).
    pub hibernate_after_minutes: i64,
    /// Golden-image builder with this installer attached.
    pub installer: Option<std::path::PathBuf>,
    /// Disks, snapshots and saved memory are encrypted with the owner's key.
    pub encrypted: bool,
}

/// Why a Mac couldn't be made ready.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ReadyError {
    /// Still booting after the caller's budget.
    #[error("the Mac is still starting")]
    Starting,
    /// Couldn't start (budget, missing image, crash).
    #[error("{0}")]
    Failed(String),
    /// It's encrypted and its owner hasn't signed in since the server started.
    #[error("{LOCKED}")]
    Locked,
}

enum Command {
    Start(Option<oneshot::Sender<Result<(), String>>>),
    Stop {
        force: bool,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Restart(oneshot::Sender<Result<(), String>>),
    Sleep(oneshot::Sender<Result<(), String>>),
    Hibernate(oneshot::Sender<Result<(), String>>),
    Wake,
    Snapshot {
        tag: String,
        reply: oneshot::Sender<Result<bool, String>>,
    },
    Restore {
        tag: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    DeleteSnapshot {
        tag: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Reload,
    Encrypt(oneshot::Sender<Result<(), String>>),
    EncryptProgress(f64),
    EncryptDone(Result<(), String>),
    Unlocked,
    Shutdown(oneshot::Sender<()>),
}

/// One Mac.
pub struct Machine {
    /// Instance id.
    pub id: String,
    status_tx: watch::Sender<Status>,
    link: Mutex<Option<Arc<Link>>>,
    commands: mpsc::Sender<Command>,
    /// Limits concurrent tool calls.
    pub calls: Semaphore,
    /// Serializes input (clicks, typing) so actions never interleave.
    pub input: tokio::sync::Mutex<()>,
    last_activity: AtomicI64,
    viewers: AtomicUsize,
    /// Memory QEMU was launched with (0 when not a local VM), for the budget.
    launched_mb: std::sync::atomic::AtomicU64,
    /// Last full-screen screenshot geometry, for clicks in screenshot pixels.
    pub last_screenshot: Mutex<Option<dot_wire::Rect>>,
    /// Pixel-to-point factor of that screenshot.
    pub last_scale: Mutex<f64>,
    /// Recent portal thumbnail (taken at, bytes, mime), shared by every open tab.
    pub thumbnail: tokio::sync::Mutex<Option<(std::time::Instant, Vec<u8>, String)>>,
    /// Owner and whether it's encrypted, once its settings are loaded.
    owner: Mutex<Option<(String, bool)>>,
    keys: Arc<Keyring>,
    /// 0 to 1 while being encrypted.
    encrypt_progress: Mutex<Option<f64>>,
}

impl Machine {
    /// Current status.
    pub fn status(&self) -> Status {
        self.status_tx.borrow().clone()
    }
    /// Watch status changes.
    pub fn watch(&self) -> watch::Receiver<Status> {
        self.status_tx.subscribe()
    }
    /// The live link, if the agent is connected.
    pub fn link(&self) -> Option<Arc<Link>> {
        self.link
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .filter(|l| l.is_alive())
    }
    /// Encrypted, and its owner's key isn't unlocked: it can't start, wake from
    /// hibernation, hibernate or take snapshots until they sign in.
    pub fn is_locked(&self) -> bool {
        let owner = self
            .owner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        owner.is_some_and(|(user, encrypted)| encrypted && !self.keys.is_unlocked(&user))
    }
    /// Progress (0 to 1) while its disks are being encrypted.
    pub fn encrypt_progress(&self) -> Option<f64> {
        *self
            .encrypt_progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    /// No tool calls for `seconds` and nobody watching the screen.
    pub fn is_idle(&self, seconds: i64) -> bool {
        self.viewers.load(Ordering::SeqCst) == 0
            && now() - self.last_activity.load(Ordering::Relaxed) >= seconds
    }
    /// Note activity (resets the idle timers).
    pub fn touch(&self) {
        self.last_activity.store(now(), Ordering::Relaxed);
    }
    /// A screen viewer connected; Macs with viewers never sleep.
    pub fn viewer(self: &Arc<Self>) -> ViewerGuard {
        self.viewers.fetch_add(1, Ordering::SeqCst);
        self.touch();
        ViewerGuard(self.clone())
    }

    async fn ask<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<Result<T, String>>) -> Command,
    ) -> Result<T, String> {
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(make(tx))
            .await
            .map_err(|_| "The Mac's supervisor stopped. Restart Familiar.".to_owned())?;
        rx.await
            .map_err(|_| "The Mac's supervisor stopped. Restart Familiar.".to_owned())?
    }

    /// Start (or wake) and wait for the agent, up to `budget`.
    pub async fn ensure_ready(&self, budget: Duration) -> Result<Arc<Link>, ReadyError> {
        self.touch();
        let mut rx = self.watch();
        let state = rx.borrow().state;
        if matches!(state, State::Stopped | State::Error | State::Hibernated) && self.is_locked() {
            return Err(ReadyError::Locked);
        }
        match state {
            State::Encrypting => return Err(ReadyError::Failed(ENCRYPTING.into())),
            State::Ready => {
                if let Some(link) = self.link() {
                    return Ok(link);
                }
            }
            State::Sleeping | State::Hibernated => {
                let _ = self.commands.send(Command::Wake).await;
            }
            State::Stopped | State::Error => {
                // Start replies after admission; a refusal (budget) comes back here.
                let (tx, reply) = oneshot::channel();
                let _ = self.commands.send(Command::Start(Some(tx))).await;
                match tokio::time::timeout(budget, reply).await {
                    Ok(Ok(Err(e))) if e == LOCKED => return Err(ReadyError::Locked),
                    Ok(Ok(Err(e))) => return Err(ReadyError::Failed(e)),
                    Ok(_) => {}
                    Err(_) => return Err(ReadyError::Starting),
                }
            }
            _ => {}
        }
        let wait = async {
            loop {
                let status = rx.borrow_and_update().clone();
                match status.state {
                    State::Ready => {
                        if let Some(link) = self.link() {
                            return Ok(link);
                        }
                    }
                    State::Stopped | State::Hibernated
                        if status.detail.as_deref() == Some(LOCKED_DETAIL) =>
                    {
                        return Err(ReadyError::Locked);
                    }
                    State::Error | State::Stopped => {
                        return Err(ReadyError::Failed(
                            status.detail.unwrap_or_else(|| "The Mac stopped.".into()),
                        ));
                    }
                    _ => {}
                }
                if rx.changed().await.is_err() {
                    return Err(ReadyError::Failed("The Mac's supervisor stopped.".into()));
                }
            }
        };
        tokio::time::timeout(budget, wait)
            .await
            .unwrap_or(Err(ReadyError::Starting))
    }

    /// Start.
    pub async fn start(&self) -> Result<(), String> {
        self.ask(|tx| Command::Start(Some(tx))).await
    }
    /// Shut down (ACPI) or force off.
    pub async fn stop(&self, force: bool) -> Result<(), String> {
        self.ask(|reply| Command::Stop { force, reply }).await
    }
    /// Restart.
    pub async fn restart(&self) -> Result<(), String> {
        self.ask(Command::Restart).await
    }
    /// Pause vCPUs now.
    pub async fn sleep(&self) -> Result<(), String> {
        self.ask(Command::Sleep).await
    }
    /// Save RAM and stop QEMU.
    pub async fn hibernate(&self) -> Result<(), String> {
        self.ask(Command::Hibernate).await
    }
    /// Wake from sleep or hibernation.
    pub async fn wake(&self) {
        self.touch();
        let _ = self.commands.send(Command::Wake).await;
    }
    /// Take a snapshot; returns whether it was live.
    pub async fn snapshot(&self, tag: String) -> Result<bool, String> {
        self.ask(|reply| Command::Snapshot { tag, reply }).await
    }
    /// Restore a snapshot.
    pub async fn restore(&self, tag: String) -> Result<(), String> {
        self.ask(|reply| Command::Restore { tag, reply }).await
    }
    /// Delete a snapshot.
    pub async fn delete_snapshot(&self, tag: String) -> Result<(), String> {
        self.ask(|reply| Command::DeleteSnapshot { tag, reply })
            .await
    }
    /// Re-read settings from the database.
    pub async fn reload(&self) {
        let _ = self.commands.send(Command::Reload).await;
    }
    /// Start encrypting a stopped Mac's disks; progress shows in its status.
    pub async fn encrypt(&self) -> Result<(), String> {
        self.ask(Command::Encrypt).await
    }
}

/// Decrements the viewer count on drop.
pub struct ViewerGuard(Arc<Machine>);

impl Drop for ViewerGuard {
    fn drop(&mut self) {
        self.0.viewers.fetch_sub(1, Ordering::SeqCst);
        self.0.touch();
    }
}

/// All machines.
#[derive(Clone)]
pub struct Fleet {
    inner: Arc<FleetInner>,
}

struct FleetInner {
    config: Arc<Config>,
    db: Db,
    key: MasterKey,
    keys: Arc<Keyring>,
    events: Events,
    hv: Arc<Hv>,
    machines: Mutex<HashMap<String, Arc<Machine>>>,
    admission: tokio::sync::Mutex<()>,
}

/// The hypervisor in use (enum dispatch keeps the trait's async fns simple).
pub enum Hv {
    /// Production.
    Qemu(qemu::Qemu),
    /// Tests.
    #[cfg(test)]
    Fake(tests::FakeHv),
}

macro_rules! dispatch {
    ($self:ident, $method:ident ( $($arg:expr),* )) => {
        match $self {
            Hv::Qemu(h) => h.$method($($arg),*).await,
            #[cfg(test)]
            Hv::Fake(h) => h.$method($($arg),*).await,
        }
    };
}

impl Hv {
    async fn provision(
        &self,
        f: &qemu::Files,
        gb: u32,
        base: bool,
        c: &Crypt,
    ) -> anyhow::Result<()> {
        dispatch!(self, provision(f, gb, base, c))
    }
    async fn launch(
        &self,
        f: &qemu::Files,
        s: &qemu::Spec,
        m: LaunchMode,
        c: &Crypt,
    ) -> anyhow::Result<()> {
        dispatch!(self, launch(f, s, m, c))
    }
    async fn status(&self, f: &qemu::Files) -> Option<String> {
        dispatch!(self, status(f))
    }
    async fn activity(&self, f: &qemu::Files) -> Option<Activity> {
        dispatch!(self, activity(f))
    }
    async fn powerdown(&self, f: &qemu::Files) -> anyhow::Result<()> {
        dispatch!(self, powerdown(f))
    }
    async fn kill(&self, f: &qemu::Files) -> anyhow::Result<()> {
        dispatch!(self, kill(f))
    }
    async fn reset(&self, f: &qemu::Files) -> anyhow::Result<()> {
        dispatch!(self, reset(f))
    }
    async fn pause(&self, f: &qemu::Files) -> anyhow::Result<()> {
        dispatch!(self, pause(f))
    }
    async fn resume(&self, f: &qemu::Files) -> anyhow::Result<()> {
        dispatch!(self, resume(f))
    }
    async fn hibernate(&self, f: &qemu::Files, c: &Crypt) -> anyhow::Result<()> {
        dispatch!(self, hibernate(f, c))
    }
    async fn snapshot(&self, f: &qemu::Files, t: &str, l: bool, c: &Crypt) -> anyhow::Result<()> {
        dispatch!(self, snapshot(f, t, l, c))
    }
    async fn restore(
        &self,
        f: &qemu::Files,
        t: &str,
        r: bool,
        c: &Crypt,
        s: &qemu::Spec,
    ) -> anyhow::Result<()> {
        dispatch!(self, restore(f, t, r, c, s))
    }
    async fn delete_snapshot(
        &self,
        f: &qemu::Files,
        t: &str,
        r: bool,
        c: &Crypt,
    ) -> anyhow::Result<()> {
        dispatch!(self, delete_snapshot(f, t, r, c))
    }
    async fn encrypt(
        &self,
        f: &qemu::Files,
        k: &MacKey,
        p: &(dyn Fn(f64) + Send + Sync),
    ) -> anyhow::Result<()> {
        dispatch!(self, encrypt(f, k, p))
    }
    /// Bytes on disk.
    pub async fn disk_bytes(&self, f: &qemu::Files) -> Option<u64> {
        dispatch!(self, disk_bytes(f))
    }
    /// Memory held by live snapshots.
    pub async fn snapshot_state_bytes(&self, f: &qemu::Files) -> Option<u64> {
        dispatch!(self, snapshot_state_bytes(f))
    }
    /// Memory held by each live snapshot, by tag.
    pub async fn snapshot_sizes(&self, f: &qemu::Files) -> Option<HashMap<String, u64>> {
        dispatch!(self, snapshot_sizes(f))
    }
}

impl Fleet {
    /// Create the fleet (no machines yet; call [`Fleet::adopt_all`]).
    pub fn new(config: Arc<Config>, db: Db, key: MasterKey, events: Events, hv: Hv) -> Self {
        Self {
            inner: Arc::new(FleetInner {
                config,
                db,
                key,
                keys: Arc::default(),
                events,
                hv: Arc::new(hv),
                machines: Mutex::default(),
                admission: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// The hypervisor.
    pub fn hv(&self) -> &Hv {
        &self.inner.hv
    }

    /// Unlocked data keys.
    pub fn keys(&self) -> &Keyring {
        &self.inner.keys
    }

    /// A person's key was just unlocked: their locked Macs pick up where they were
    /// (ones that were running start again; hibernated ones wake on demand).
    pub async fn unlocked(&self, user_id: &str) {
        let uid = user_id.to_owned();
        let ids: Vec<String> = self
            .inner
            .db
            .call(move |c| {
                let mut s =
                    c.prepare("SELECT id FROM instances WHERE user_id = ?1 AND encrypted = 1")?;
                let rows = s.query_map([uid], |r| r.get(0))?;
                rows.collect()
            })
            .await
            .unwrap_or_default();
        for id in ids {
            let _ = self.machine(&id).commands.send(Command::Unlocked).await;
        }
    }

    /// Instance files.
    pub fn files(&self, id: &str) -> qemu::Files {
        qemu::Files::new(&self.inner.config.data_dir, id)
    }

    /// Start supervising every instance in the database. Running VMs are adopted.
    pub async fn adopt_all(&self) -> anyhow::Result<()> {
        let ids: Vec<String> = self
            .inner
            .db
            .call(|c| {
                let mut s = c.prepare("SELECT id FROM instances")?;
                let rows = s.query_map([], |r| r.get(0))?;
                rows.collect()
            })
            .await?;
        for id in ids {
            self.machine(&id);
        }
        Ok(())
    }

    /// The machine for an instance, starting its supervisor on first use.
    pub fn machine(&self, id: &str) -> Arc<Machine> {
        let mut machines = self
            .inner
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(m) = machines.get(id) {
            return m.clone();
        }
        let (tx, rx) = mpsc::channel(32);
        let machine = Arc::new(Machine {
            id: id.to_owned(),
            status_tx: watch::channel(Status {
                state: State::Stopped,
                detail: None,
            })
            .0,
            link: Mutex::new(None),
            commands: tx,
            calls: Semaphore::new(self.inner.config.limits.calls_per_instance),
            input: tokio::sync::Mutex::new(()),
            last_activity: AtomicI64::new(now()),
            viewers: AtomicUsize::new(0),
            launched_mb: std::sync::atomic::AtomicU64::new(0),
            last_screenshot: Mutex::new(None),
            last_scale: Mutex::new(1.0),
            thumbnail: tokio::sync::Mutex::new(None),
            owner: Mutex::new(None),
            keys: self.inner.keys.clone(),
            encrypt_progress: Mutex::new(None),
        });
        machines.insert(id.to_owned(), machine.clone());
        let supervisor = Supervisor {
            fleet: self.clone(),
            machine: machine.clone(),
            files: self.files(id),
            settings: None,
            failures: Vec::new(),
            link_down_since: None,
            last_beat: std::time::Instant::now(),
            missed: 0,
            connect_backoff: Duration::from_millis(500),
            next_connect: std::time::Instant::now(),
            intentional_stop: false,
            restart_at: None,
            freeze: FreezeWatch::default(),
            locked_out: false,
        };
        tokio::spawn(supervisor.run(rx));
        machine
    }

    /// Stop supervising (instance deleted). QEMU must already be stopped.
    pub async fn forget(&self, id: &str) {
        let machine = self
            .inner
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id);
        if let Some(m) = machine {
            let (tx, rx) = oneshot::channel();
            let _ = m.commands.send(Command::Shutdown(tx)).await;
            let _ = tokio::time::timeout(Duration::from_secs(5), rx).await;
        }
    }

    /// Snapshot of every machine.
    pub fn all(&self) -> Vec<Arc<Machine>> {
        self.inner
            .machines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    /// The RAM budget in MB.
    pub fn memory_budget_mb(&self) -> u64 {
        let configured = self.inner.config.limits.memory_budget_mb;
        if configured > 0 {
            return configured;
        }
        crate::host::memory()
            .map(|(total, _)| total * 3 / 4)
            .unwrap_or(16_384)
    }

    /// The RAM budget in MB as the admin set it in the portal (or the config default).
    pub async fn effective_memory_budget_mb(&self) -> u64 {
        self.budget_settings().await.0
    }

    /// Free space (GB) Macs must leave on the server: the admin's `reserve_gb`, or the
    /// config file's `limits.min_free_disk_gb`.
    pub async fn reserve_gb(&self) -> u64 {
        self.inner
            .db
            .setting(crate::storage::RESERVE_KEY)
            .await
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(self.inner.config.limits.min_free_disk_gb)
    }

    async fn budget_settings(&self) -> (u64, u32) {
        let db = &self.inner.db;
        let mem = db
            .setting("memory_budget_mb")
            .await
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| self.memory_budget_mb());
        let max = db
            .setting("max_running")
            .await
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(self.inner.config.limits.max_running);
        (mem, max)
    }

    /// RAM and count in use by running machines (excluding `except`).
    pub async fn usage(&self, except: Option<&str>) -> (u64, u32) {
        let mut mem = 0u64;
        let mut count = 0u32;
        for m in self.all() {
            // Remote Macs use no memory here; launched_mb is 0 for them.
            let mb = m.launched_mb.load(Ordering::SeqCst);
            if Some(m.id.as_str()) == except || mb == 0 || !m.status().state.uses_memory() {
                continue;
            }
            mem += mb;
            count += 1;
        }
        (mem, count)
    }

    /// Make room for `memory_mb`, hibernating idle Macs if needed.
    async fn admit(
        &self,
        id: &str,
        memory_mb: u64,
    ) -> Result<tokio::sync::MutexGuard<'_, ()>, String> {
        let guard = self.inner.admission.lock().await;
        let (budget, max_running) = self.budget_settings().await;
        if memory_mb > budget {
            return Err(format!(
                "This Mac needs {} GB of memory but the server allows {} GB in total. Lower its memory in Settings.",
                memory_mb / 1024,
                budget / 1024
            ));
        }
        let mut tried: Vec<String> = Vec::new();
        for _ in 0..8 {
            let (used, running) = self.usage(Some(id)).await;
            if used + memory_mb <= budget && running < max_running {
                return Ok(guard);
            }
            // Hibernate the least recently used idle Mac to make room.
            let victim = self
                .all()
                .into_iter()
                .filter(|m| {
                    m.id != id
                        && m.launched_mb.load(Ordering::SeqCst) > 0
                        && !tried.contains(&m.id)
                        && matches!(m.status().state, State::Ready | State::Sleeping)
                        && !m.is_locked()
                        && m.viewers.load(Ordering::SeqCst) == 0
                        && now() - m.last_activity.load(Ordering::Relaxed) > 120
                })
                .min_by_key(|m| m.last_activity.load(Ordering::Relaxed));
            let Some(victim) = victim else {
                return Err(
                    "The server is at its limit for running Macs and none are idle. Try again in a few minutes, or stop another Mac."
                        .into(),
                );
            };
            tracing::info!(instance = %victim.id, "hibernating to make room");
            tried.push(victim.id.clone());
            if let Err(e) = victim.hibernate().await {
                tracing::warn!(instance = %victim.id, error = %e, "couldn't hibernate to make room");
            }
        }
        Err("Couldn't free enough memory to start this Mac.".into())
    }

    /// Hibernate or keep every Mac when the server stops. A restart for an update always
    /// leaves them running: the new server adopts them a second later.
    pub async fn shutdown(&self, restarting: bool) {
        if !restarting && self.inner.config.vm.on_shutdown == crate::config::OnShutdown::Hibernate {
            let running: Vec<_> = self
                .all()
                .into_iter()
                .filter(|m| m.status().state.uses_memory())
                .collect();
            tracing::info!(count = running.len(), "hibernating Macs before stopping");
            let saves = running.iter().map(|m| m.hibernate());
            for (m, r) in running
                .iter()
                .zip(futures_util::future::join_all(saves).await)
            {
                if let Err(e) = r {
                    // Includes encrypted Macs whose owner hasn't signed in since the last
                    // start: their memory can't be saved without the key.
                    tracing::error!(instance = %m.id, error = %e, "couldn't hibernate; it will cold boot next time");
                }
            }
        }
        for m in self.all() {
            let (tx, rx) = oneshot::channel();
            if m.commands.send(Command::Shutdown(tx)).await.is_ok() {
                let _ = tokio::time::timeout(Duration::from_secs(5), rx).await;
            }
        }
    }
}

struct Supervisor {
    fleet: Fleet,
    machine: Arc<Machine>,
    files: qemu::Files,
    settings: Option<Settings>,
    failures: Vec<std::time::Instant>,
    link_down_since: Option<std::time::Instant>,
    last_beat: std::time::Instant,
    missed: u32,
    connect_backoff: Duration,
    next_connect: std::time::Instant,
    intentional_stop: bool,
    restart_at: Option<std::time::Instant>,
    freeze: FreezeWatch,
    /// A start or wake was refused because the owner's key is locked.
    locked_out: bool,
}

const TICK: Duration = Duration::from_secs(2);
const HEARTBEAT: Duration = Duration::from_secs(5);
/// The longest a Mac that's shutting down may look frozen before it's turned off.
const FROZEN_WHILE_STOPPING: u64 = 20;
/// Restart delays after unexpected QEMU exits; after these, the Mac is held in `error`.
const BACKOFF: [u64; 3] = [5, 30, 120];

impl Supervisor {
    fn inner(&self) -> &FleetInner {
        &self.fleet.inner
    }

    fn set(&self, state: State, detail: Option<&str>) {
        let next = Status {
            state,
            detail: detail.map(str::to_owned),
        };
        let changed = self.machine.status_tx.send_if_modified(|s| {
            if *s == next {
                false
            } else {
                *s = next.clone();
                true
            }
        });
        if changed {
            tracing::info!(instance = %self.machine.id, ?state, detail, "state");
            self.inner().events.instance(
                &self.machine.id,
                "state",
                serde_json::json!({ "state": state, "detail": detail }),
            );
        }
    }

    async fn load_settings(&mut self) -> anyhow::Result<()> {
        let id = self.machine.id.clone();
        let key = self.inner().key.clone();
        let data_dir = self.inner().config.data_dir.clone();
        let row = self
            .inner()
            .db
            .call(move |c| {
                c.query_row(
                    "SELECT user_id, driver, cpus, memory_mb, disk_gb, port, remote_host, secret, desired,
                            sleep_after_minutes, hibernate_after_minutes, is_builder, last_active_at, encrypted
                     FROM instances WHERE id = ?1",
                    [&id],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, u32>(2)?,
                            r.get::<_, u32>(3)?,
                            r.get::<_, u32>(4)?,
                            r.get::<_, Option<u16>>(5)?,
                            r.get::<_, Option<String>>(6)?,
                            r.get::<_, Vec<u8>>(7)?,
                            r.get::<_, String>(8)?,
                            r.get::<_, i64>(9)?,
                            r.get::<_, i64>(10)?,
                            r.get::<_, bool>(11)?,
                            r.get::<_, Option<i64>>(12)?,
                            r.get::<_, bool>(13)?,
                        ))
                    },
                )
            })
            .await?;
        let secret = key.open(&format!("guest:{}", self.machine.id), &row.7)?;
        if let Some(last) = row.12 {
            self.machine
                .last_activity
                .fetch_max(last, Ordering::Relaxed);
        }
        // Builders become the shared base image, so they're never encrypted.
        let encrypted = row.13 && !row.11;
        *self
            .machine
            .owner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((row.0.clone(), encrypted));
        self.settings = Some(Settings {
            user_id: row.0,
            driver: row.1,
            cpus: row.2,
            memory_mb: row.3,
            disk_gb: row.4,
            port: row.5,
            remote_host: row.6,
            secret,
            desired_running: row.8 == "running",
            sleep_after_minutes: row.9,
            hibernate_after_minutes: row.10,
            installer: row
                .11
                .then(|| data_dir.join("images").join("BaseSystem.img")),
            encrypted,
        });
        Ok(())
    }

    /// The key this Mac's files need right now.
    fn crypt(&self) -> Crypt {
        match &self.settings {
            Some(s) if s.encrypted => match self.inner().keys.get(&s.user_id) {
                Some(key) => Crypt::Key(key.mac_key(&self.machine.id)),
                None => Crypt::Locked,
            },
            _ => Crypt::Plain,
        }
    }

    fn spec(&self, settings: &Settings) -> anyhow::Result<qemu::Spec> {
        Ok(qemu::Spec {
            id: self.machine.id.clone(),
            cpus: settings.cpus,
            memory_mb: settings.memory_mb,
            port: settings
                .port
                .ok_or_else(|| anyhow::anyhow!("no port assigned"))?,
            installer: settings.installer.clone(),
        })
    }

    async fn set_desired(&self, running: bool) {
        let id = self.machine.id.clone();
        let value = if running { "running" } else { "stopped" };
        let _ = self
            .inner()
            .db
            .call(move |c| {
                c.execute(
                    "UPDATE instances SET desired = ?2 WHERE id = ?1",
                    (id, value),
                )
            })
            .await;
    }

    fn is_qemu(&self) -> bool {
        self.settings.as_ref().is_some_and(|s| s.driver == "qemu")
    }

    fn drop_link(&mut self) {
        *self
            .machine
            .link
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    async fn run(mut self, mut rx: mpsc::Receiver<Command>) {
        if let Err(e) = self.load_settings().await {
            tracing::error!(instance = %self.machine.id, error = %e, "can't load instance");
            self.set(State::Error, Some("This Mac's settings couldn't be read."));
        } else {
            self.adopt().await;
        }
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    Some(Command::Shutdown(done)) => { let _ = done.send(()); break; }
                    Some(cmd) => self.handle(cmd).await,
                    None => break,
                },
                _ = tick.tick() => self.tick().await,
            }
        }
    }

    /// On startup, pick up where things were: adopt running VMs, restore hibernated ones lazily.
    async fn adopt(&mut self) {
        let Some(settings) = self.settings.clone() else {
            return;
        };
        if settings.driver == "remote" {
            if settings.desired_running {
                self.set(State::Booting, Some("Connecting to the Mac"));
            }
            return;
        }
        let status = self.inner().hv.status(&self.files).await;
        if status.is_some() {
            // An adopted VM still holds the memory it was started with.
            self.machine
                .launched_mb
                .store(u64::from(settings.memory_mb), Ordering::SeqCst);
        }
        match status.as_deref() {
            Some("paused") => self.set(
                State::Sleeping,
                Some("Asleep. It wakes when your assistant needs it."),
            ),
            Some(_) => self.set(State::Booting, Some("Reconnecting to the Mac")),
            None if self.files.hibernate().exists() => {
                // An encrypted Mac stays hibernated until its owner signs in.
                let locked = matches!(self.crypt(), Crypt::Locked);
                self.locked_out = locked;
                self.set(
                    State::Hibernated,
                    Some(if locked {
                        LOCKED_DETAIL
                    } else {
                        HIBERNATED_DETAIL
                    }),
                );
            }
            None if settings.desired_running => {
                self.set(State::Stopped, None);
                self.start().await.ok();
            }
            None => self.set(State::Stopped, None),
        }
    }

    async fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Start(reply) => {
                let r = self.start().await;
                if let Some(tx) = reply {
                    let _ = tx.send(r);
                }
            }
            Command::Stop { force, reply } => {
                let _ = reply.send(self.stop(force).await);
            }
            Command::Restart(reply) => {
                let r = match self.stop(false).await {
                    Ok(()) => self.start().await,
                    Err(e) => Err(e),
                };
                let _ = reply.send(r);
            }
            Command::Sleep(reply) => {
                let _ = reply.send(self.sleep().await);
            }
            Command::Hibernate(reply) => {
                let _ = reply.send(self.hibernate().await);
            }
            Command::Wake => self.wake().await,
            Command::Snapshot { tag, reply } => {
                let _ = reply.send(self.snapshot(&tag).await);
            }
            Command::Restore { tag, reply } => {
                let _ = reply.send(self.restore(&tag).await);
            }
            Command::DeleteSnapshot { tag, reply } => {
                let running = self.machine.status().state.uses_memory();
                let crypt = self.crypt();
                let r = self
                    .inner()
                    .hv
                    .delete_snapshot(&self.files, &tag, running, &crypt)
                    .await
                    .map_err(|e| format!("{e:#}"));
                let _ = reply.send(r);
            }
            Command::Reload => {
                if let Err(e) = self.load_settings().await {
                    tracing::warn!(instance = %self.machine.id, error = %e, "reload failed");
                }
            }
            Command::Encrypt(reply) => {
                let _ = reply.send(self.encrypt());
            }
            Command::EncryptProgress(p) => {
                if self.machine.status().state == State::Encrypting {
                    *self
                        .machine
                        .encrypt_progress
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(p);
                    self.set(
                        State::Encrypting,
                        Some(&format!("Encrypting… {}%", (p * 100.0).floor())),
                    );
                }
            }
            Command::EncryptDone(result) => {
                *self
                    .machine
                    .encrypt_progress
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                let _ = self.load_settings().await;
                match result {
                    Ok(()) => self.set(State::Stopped, None),
                    Err(e) => {
                        let sentence = format!(
                            "Encrypting didn't finish, so this Mac is unchanged: {}",
                            crate::util::truncate(&e, 200)
                        );
                        self.set(State::Stopped, Some(&sentence));
                    }
                }
            }
            Command::Unlocked => {
                if self.locked_out {
                    self.locked_out = false;
                    let _ = self.load_settings().await;
                    match self.machine.status().state {
                        State::Hibernated => self.set(State::Hibernated, Some(HIBERNATED_DETAIL)),
                        State::Stopped
                            if self.settings.as_ref().is_some_and(|s| s.desired_running) =>
                        {
                            let _ = self.start().await;
                        }
                        State::Stopped => self.set(State::Stopped, None),
                        _ => {}
                    }
                }
            }
            Command::Shutdown(_) => {}
        }
    }

    async fn start(&mut self) -> Result<(), String> {
        let Some(settings) = self.settings.clone() else {
            return Err("This Mac's settings couldn't be read.".into());
        };
        self.failures.clear();
        self.set_desired(true).await;
        self.intentional_stop = false;
        let state = self.machine.status().state;
        if state == State::Encrypting {
            return Err(ENCRYPTING.into());
        }
        if matches!(
            state,
            State::Ready | State::Booting | State::Starting | State::Degraded
        ) {
            return Ok(());
        }
        if matches!(state, State::Sleeping | State::Hibernated) {
            self.wake().await;
            return Ok(());
        }
        if settings.driver == "remote" {
            self.set(State::Booting, Some("Connecting to the Mac"));
            self.next_connect = std::time::Instant::now();
            return Ok(());
        }
        self.launch(LaunchMode::Fresh).await
    }

    async fn launch(&mut self, mode: LaunchMode) -> Result<(), String> {
        let Some(settings) = self.settings.clone() else {
            return Err("This Mac's settings couldn't be read.".into());
        };
        let crypt = self.crypt();
        if matches!(crypt, Crypt::Locked) {
            self.locked_out = true;
            let prior = if mode == LaunchMode::Resume {
                State::Hibernated
            } else {
                State::Stopped
            };
            self.set(prior, Some(LOCKED_DETAIL));
            return Err(LOCKED.into());
        }
        let fleet = self.fleet.clone();
        let _admitted = match fleet
            .admit(&self.machine.id, u64::from(settings.memory_mb))
            .await
        {
            Ok(g) => g,
            Err(e) => {
                let prior = if mode == LaunchMode::Resume {
                    State::Hibernated
                } else {
                    State::Stopped
                };
                self.set(prior, Some(&e));
                return Err(e);
            }
        };
        if let Some(free) = crate::host::disk_free(&self.inner().config.data_dir) {
            let min = fleet.reserve_gb().await.saturating_mul(crate::storage::GB);
            if free < min {
                let msg = "The server is almost out of disk space, so Macs can't start. An admin needs to free space.";
                self.set(State::Error, Some(msg));
                return Err(msg.into());
            }
        }
        self.set(
            State::Starting,
            Some(if mode == LaunchMode::Resume {
                "Restoring from hibernation"
            } else {
                "Starting the virtual machine"
            }),
        );
        let result = async {
            tokio::fs::create_dir_all(&self.files.dir).await?;
            if mode == LaunchMode::Fresh {
                // A cold boot makes any saved memory stale; never resume it over a changed disk.
                let _ = tokio::fs::remove_file(self.files.hibernate()).await;
            }
            let builder = settings.installer.is_some();
            self.inner()
                .hv
                .provision(&self.files, settings.disk_gb, !builder, &crypt)
                .await?;
            configdisk::write(&self.files.config_img(), &self.machine.id, &settings.secret)?;
            let spec = self.spec(&settings)?;
            self.inner()
                .hv
                .launch(&self.files, &spec, mode, &crypt)
                .await
        }
        .await;
        match result {
            Ok(()) => {
                self.machine
                    .launched_mb
                    .store(u64::from(settings.memory_mb), Ordering::SeqCst);
                let now_s = now();
                let id = self.machine.id.clone();
                let _ = self
                    .inner()
                    .db
                    .call(move |c| {
                        c.execute(
                            "UPDATE instances SET booted_at = ?2 WHERE id = ?1",
                            (id, now_s),
                        )
                    })
                    .await;
                self.set(
                    State::Booting,
                    Some(if mode == LaunchMode::Resume {
                        "Waking up"
                    } else {
                        "Waiting for macOS to finish starting"
                    }),
                );
                self.link_down_since = Some(std::time::Instant::now());
                self.next_connect = std::time::Instant::now()
                    + Duration::from_secs(if mode == LaunchMode::Resume { 1 } else { 8 });
                self.connect_backoff = Duration::from_millis(500);
                Ok(())
            }
            Err(e) => {
                let msg = format!("{e:#}");
                tracing::error!(instance = %self.machine.id, error = %msg, "launch failed");
                let sentence = format!(
                    "The Mac couldn't start: {}",
                    crate::util::truncate(&msg, 240)
                );
                self.set(State::Error, Some(&sentence));
                Err(sentence)
            }
        }
    }

    async fn stop(&mut self, force: bool) -> Result<(), String> {
        if self.machine.status().state == State::Encrypting {
            return Err("Wait for this Mac to finish encrypting.".into());
        }
        self.restart_at = None;
        self.set_desired(false).await;
        self.intentional_stop = true;
        self.drop_link();
        if !self.is_qemu() {
            self.set(State::Stopped, None);
            return Ok(());
        }
        let hv = self.inner().hv.clone();
        if hv.status(&self.files).await.is_none() {
            let _ = tokio::fs::remove_file(self.files.hibernate()).await;
            self.set(State::Stopped, None);
            return Ok(());
        }
        self.set(
            State::Stopping,
            Some(if force {
                "Turning off"
            } else {
                "Shutting down macOS"
            }),
        );
        if !force {
            // A paused VM can't react to the power button.
            let _ = hv.resume(&self.files).await;
            let _ = hv.powerdown(&self.files).await;
            let deadline = std::time::Instant::now() + Duration::from_secs(90);
            let mut freeze = FreezeWatch::default();
            let frozen_after = self
                .inner()
                .config
                .vm
                .frozen_reset_seconds
                .min(FROZEN_WHILE_STOPPING);
            while std::time::Instant::now() < deadline {
                if hv.status(&self.files).await.is_none() {
                    break;
                }
                // macOS has already flushed its disks by the time a failed halt freezes it.
                if frozen_after > 0
                    && freeze.observe(
                        std::time::Instant::now(),
                        hv.activity(&self.files).await,
                        Duration::from_secs(frozen_after),
                    )
                {
                    tracing::warn!(instance = %self.machine.id, "macOS froze while shutting down; turning it off");
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
        if hv.status(&self.files).await.is_some() {
            let _ = hv.kill(&self.files).await;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let _ = tokio::fs::remove_file(self.files.hibernate()).await;
        self.machine.launched_mb.store(0, Ordering::SeqCst);
        self.set(State::Stopped, None);
        Ok(())
    }

    async fn sleep(&mut self) -> Result<(), String> {
        if !self.is_qemu() {
            return Err("Only Macs on this server can sleep.".into());
        }
        if !matches!(
            self.machine.status().state,
            State::Ready | State::Degraded | State::Booting
        ) {
            return Ok(());
        }
        self.drop_link();
        self.inner()
            .hv
            .pause(&self.files)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.set(
            State::Sleeping,
            Some("Asleep. It wakes when your assistant needs it."),
        );
        Ok(())
    }

    async fn hibernate(&mut self) -> Result<(), String> {
        if !self.is_qemu() {
            return Err("Only Macs on this server can hibernate.".into());
        }
        if !self.machine.status().state.uses_memory() {
            return Ok(());
        }
        // Saved memory is encrypted with the owner's key; without it, keep running.
        let crypt = self.crypt();
        if matches!(crypt, Crypt::Locked) {
            return Err(LOCKED.into());
        }
        self.drop_link();
        self.intentional_stop = true;
        self.set(State::Stopping, Some("Saving memory to disk"));
        match self.inner().hv.hibernate(&self.files, &crypt).await {
            Ok(()) => {
                self.set(State::Hibernated, Some(HIBERNATED_DETAIL));
                Ok(())
            }
            Err(e) => {
                self.intentional_stop = false;
                self.set(State::Booting, Some("Reconnecting to the Mac"));
                Err(format!("{e:#}"))
            }
        }
    }

    /// Check that this Mac can be encrypted, then convert its disks in the background.
    /// The supervisor keeps answering meanwhile; `Encrypting` refuses starts.
    fn encrypt(&mut self) -> Result<(), String> {
        let Some(settings) = self.settings.clone() else {
            return Err("This Mac's settings couldn't be read.".into());
        };
        if settings.driver != "qemu" || settings.installer.is_some() {
            return Err("Only Macs on this server can be encrypted.".into());
        }
        if settings.encrypted {
            return Err("This Mac is already encrypted.".into());
        }
        if !matches!(self.machine.status().state, State::Stopped | State::Error) {
            return Err("Shut down this Mac first.".into());
        }
        let Some(key) = self.inner().keys.get(&settings.user_id) else {
            return Err(LOCKED.into());
        };
        let mac_key = key.mac_key(&self.machine.id);
        *self
            .machine
            .encrypt_progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(0.0);
        self.set(State::Encrypting, Some("Encrypting… 0%"));
        let fleet = self.fleet.clone();
        let files = self.files.clone();
        let machine = self.machine.clone();
        tokio::spawn(async move {
            let commands = machine.commands.clone();
            let last = Mutex::new(-1.0f64);
            let progress = |p: f64| {
                // Whole percents are plenty; the channel isn't flooded.
                let mut last = last
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if (p * 100.0).floor() > (*last * 100.0).floor() {
                    *last = p;
                    let _ = commands.try_send(Command::EncryptProgress(p));
                }
            };
            let result = async {
                // A Mac that never started has no disks yet; it's simply encrypted from now on.
                if files.disk().exists() {
                    fleet.inner.hv.encrypt(&files, &mac_key, &progress).await?;
                }
                let id = machine.id.clone();
                fleet
                    .inner
                    .db
                    .call(move |c| {
                        let tx = c.transaction()?;
                        tx.execute("UPDATE instances SET encrypted = 1 WHERE id = ?1", [&id])?;
                        // Converting doesn't carry internal snapshots over.
                        tx.execute("DELETE FROM snapshots WHERE instance_id = ?1", [&id])?;
                        tx.commit()
                    })
                    .await?;
                anyhow::Ok(())
            }
            .await
            .map_err(|e| format!("{e:#}"));
            if let Err(e) = &result {
                tracing::error!(instance = %machine.id, error = %e, "encrypting failed");
            } else {
                tracing::info!(instance = %machine.id, "encrypted");
            }
            let _ = commands.send(Command::EncryptDone(result)).await;
        });
        Ok(())
    }

    async fn wake(&mut self) {
        self.machine.touch();
        match self.machine.status().state {
            State::Sleeping => {
                if self.inner().hv.resume(&self.files).await.is_ok() {
                    self.set(State::Booting, Some("Waking up"));
                    self.next_connect = std::time::Instant::now();
                } else if self.inner().hv.status(&self.files).await.is_none() {
                    self.drop_link();
                    self.crashed().await;
                }
            }
            State::Hibernated => {
                self.set_desired(true).await;
                self.intentional_stop = false;
                let _ = self.launch(LaunchMode::Resume).await;
            }
            _ => {}
        }
    }

    async fn snapshot(&mut self, tag: &str) -> Result<bool, String> {
        if !self.is_qemu() {
            return Err("Snapshots need a Mac on this server.".into());
        }
        let reserve = self
            .fleet
            .reserve_gb()
            .await
            .saturating_mul(crate::storage::GB);
        if let Some(free) = crate::host::disk_free(&self.inner().config.data_dir)
            && free < reserve
        {
            return Err("The server is almost out of disk space, so no snapshot was taken.".into());
        }
        let state = self.machine.status().state;
        if state == State::Hibernated {
            return Err("Wake the Mac or shut it down before taking a snapshot.".into());
        }
        if state == State::Encrypting {
            return Err(ENCRYPTING.into());
        }
        let crypt = self.crypt();
        if matches!(crypt, Crypt::Locked) {
            return Err(LOCKED.into());
        }
        let live = state.uses_memory();
        if state == State::Sleeping {
            let _ = self.inner().hv.resume(&self.files).await;
        }
        let r = self
            .inner()
            .hv
            .snapshot(&self.files, tag, live, &crypt)
            .await;
        if state == State::Sleeping {
            let _ = self.inner().hv.pause(&self.files).await;
        }
        r.map(|()| live).map_err(|e| format!("{e:#}"))
    }

    async fn restore(&mut self, tag: &str) -> Result<(), String> {
        let Some(settings) = self.settings.clone().filter(|s| s.driver == "qemu") else {
            return Err("Snapshots need a Mac on this server.".into());
        };
        let crypt = self.crypt();
        if matches!(crypt, Crypt::Locked) {
            return Err(LOCKED.into());
        }
        let spec = self.spec(&settings).map_err(|e| format!("{e:#}"))?;
        let state = self.machine.status().state;
        if state == State::Encrypting {
            return Err(ENCRYPTING.into());
        }
        if state == State::Hibernated {
            let _ = tokio::fs::remove_file(self.files.hibernate()).await;
            self.set(State::Stopped, None);
        }
        let running = self.machine.status().state.uses_memory();
        self.drop_link();
        let r = self
            .inner()
            .hv
            .restore(&self.files, tag, running, &crypt, &spec)
            .await;
        if running {
            let _ = self.inner().hv.resume(&self.files).await;
            self.set(
                State::Booting,
                Some("Reconnecting after restoring the snapshot"),
            );
            self.next_connect = std::time::Instant::now();
        }
        r.map_err(|e| format!("{e:#}"))
    }

    async fn tick(&mut self) {
        let Some(settings) = self.settings.clone() else {
            return;
        };
        if let Some(at) = self.restart_at {
            if std::time::Instant::now() >= at {
                self.restart_at = None;
                if !self.intentional_stop {
                    self.set(State::Stopped, None);
                    let _ = self.launch(LaunchMode::Fresh).await;
                }
            }
            return;
        }
        let state = self.machine.status().state;

        // 1. Is QEMU still there?
        if settings.driver == "qemu"
            && matches!(
                state,
                State::Booting | State::Ready | State::Degraded | State::Sleeping
            )
        {
            match self.inner().hv.status(&self.files).await.as_deref() {
                None => {
                    self.drop_link();
                    self.crashed().await;
                    return;
                }
                Some("shutdown") => {
                    // The guest shut itself down; QEMU stays until told to quit.
                    let _ = self.inner().hv.kill(&self.files).await;
                    self.set_desired(false).await;
                    self.set(State::Stopped, Some("macOS shut down."));
                    return;
                }
                _ => {}
            }
        }

        // 2. Keep the agent link up.
        if matches!(state, State::Booting | State::Ready | State::Degraded) {
            self.maintain_link(&settings).await;
        }

        // 3. A restart that froze in the kernel never comes back on its own.
        if settings.driver == "qemu" {
            self.maybe_reset_frozen(state).await;
        }

        // 4. Idle policy.
        let idle_min = (now() - self.machine.last_activity.load(Ordering::Relaxed)) / 60;
        let viewers = self.machine.viewers.load(Ordering::SeqCst);
        if settings.driver == "qemu" && viewers == 0 {
            if state == State::Ready
                && settings.sleep_after_minutes > 0
                && idle_min >= settings.sleep_after_minutes
            {
                let _ = self.sleep().await;
            } else if matches!(state, State::Ready | State::Sleeping)
                && settings.hibernate_after_minutes > 0
                && idle_min >= settings.hibernate_after_minutes
                && !matches!(self.crypt(), Crypt::Locked)
            {
                let _ = self.hibernate().await;
            }
        }
    }

    async fn crashed(&mut self) {
        if self.intentional_stop {
            self.set(State::Stopped, None);
            return;
        }
        let now_i = std::time::Instant::now();
        self.failures
            .retain(|t| now_i.duration_since(*t) < Duration::from_secs(900));
        self.failures.push(now_i);
        let n = self.failures.len();
        if n > BACKOFF.len() {
            self.set(
                State::Error,
                Some("The Mac stopped unexpectedly several times. Check the server log, then start it again."),
            );
            return;
        }
        let wait = BACKOFF[n - 1];
        tracing::warn!(instance = %self.machine.id, attempt = n, wait, "QEMU exited unexpectedly; restarting");
        self.set(
            State::Starting,
            Some(&format!(
                "The Mac stopped unexpectedly. Restarting in {wait} seconds."
            )),
        );
        // Restarted from tick() so commands (like Stop) are still handled meanwhile.
        self.restart_at = Some(now_i + Duration::from_secs(wait));
    }

    async fn maintain_link(&mut self, settings: &Settings) {
        if let Some(link) = self.machine.link() {
            if self.last_beat.elapsed() >= HEARTBEAT {
                self.last_beat = std::time::Instant::now();
                match link.call(Op::Ping, Duration::from_secs(3)).await {
                    Ok(_) => {
                        self.missed = 0;
                        if self.machine.status().state != State::Ready {
                            self.set(State::Ready, None);
                        }
                    }
                    Err(_) => {
                        self.missed += 1;
                        if self.missed >= 3 {
                            self.set(
                                State::Degraded,
                                Some("The Mac stopped answering. Reconnecting."),
                            );
                            self.drop_link();
                            self.link_down_since = Some(std::time::Instant::now());
                        }
                    }
                }
            }
            return;
        }
        self.maybe_reset_unresponsive(settings).await;
        if std::time::Instant::now() < self.next_connect {
            return;
        }
        let addr = match settings.driver.as_str() {
            "remote" => settings.remote_host.clone().unwrap_or_default(),
            _ => format!(
                "{}:{}",
                self.inner().config.vm.forward_address,
                settings.port.unwrap_or(0)
            ),
        };
        match Link::connect(
            &addr,
            &settings.secret,
            &self.machine.id,
            Duration::from_secs(5),
        )
        .await
        {
            Ok(link) => {
                tracing::info!(instance = %self.machine.id, agent = %link.agent_version, "agent connected");
                self.store_guest_info(&link.info).await;
                *self
                    .machine
                    .link
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(link);
                self.missed = 0;
                self.link_down_since = None;
                self.connect_backoff = Duration::from_millis(500);
                self.last_beat = std::time::Instant::now();
                self.set(State::Ready, None);
            }
            Err(e) => {
                tracing::debug!(instance = %self.machine.id, error = %e, "agent not reachable yet");
                if e.downcast_ref::<dot_wire::handshake::HandshakeError>()
                    .is_some()
                {
                    // The Mac answered, so it isn't hung: don't let the reset timer fire.
                    self.link_down_since = Some(std::time::Instant::now());
                    self.set(
                        State::Degraded,
                        Some("The Mac's agent refused the connection. Its pairing doesn't match this server."),
                    );
                }
                self.next_connect = std::time::Instant::now() + self.connect_backoff;
                self.connect_backoff = (self.connect_backoff * 2).min(Duration::from_secs(5));
            }
        }
    }

    async fn maybe_reset_frozen(&mut self, state: State) {
        let seconds = self.inner().config.vm.frozen_reset_seconds;
        // A connected agent proves the guest is alive, and spares it the sampling.
        if seconds == 0
            || !matches!(state, State::Booting | State::Degraded)
            || self.machine.link().is_some()
        {
            self.freeze = FreezeWatch::default();
            return;
        }
        let sample = self.inner().hv.activity(&self.files).await;
        if !self.freeze.observe(
            std::time::Instant::now(),
            sample,
            Duration::from_secs(seconds),
        ) {
            return;
        }
        // Restart and shut down freeze alike, and QEMU can't tell which was asked for.
        tracing::warn!(instance = %self.machine.id, "macOS froze in the kernel; resetting the VM");
        self.freeze = FreezeWatch::default();
        let _ = self.inner().hv.reset(&self.files).await;
        self.link_down_since = Some(std::time::Instant::now());
        self.set(
            State::Booting,
            Some("macOS froze while restarting, so it was restarted."),
        );
    }

    async fn maybe_reset_unresponsive(&mut self, settings: &Settings) {
        let minutes = self.inner().config.vm.unresponsive_reset_minutes;
        if settings.driver != "qemu" || minutes == 0 || settings.installer.is_some() {
            return;
        }
        let Some(since) = self.link_down_since else {
            self.link_down_since = Some(std::time::Instant::now());
            return;
        };
        // Cold boots of macOS can take a few minutes; give booting twice as long.
        let limit = if self.machine.status().state == State::Booting {
            minutes * 2
        } else {
            minutes
        };
        if since.elapsed() >= Duration::from_secs(limit * 60) {
            tracing::warn!(instance = %self.machine.id, "agent unresponsive; resetting the VM");
            let _ = self.inner().hv.reset(&self.files).await;
            self.link_down_since = Some(std::time::Instant::now());
            self.set(
                State::Booting,
                Some("The Mac stopped responding, so it was restarted."),
            );
        }
    }

    async fn store_guest_info(&self, info: &GuestInfo) {
        let Ok(json) = serde_json::to_string(info) else {
            return;
        };
        let id = self.machine.id.clone();
        let _ = self
            .inner()
            .db
            .call(move |c| {
                c.execute(
                    "UPDATE instances SET guest_info = ?2 WHERE id = ?1",
                    (id, json),
                )
            })
            .await;
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::db::Db;
    use std::sync::Mutex as StdMutex;

    /// A hypervisor that keeps per-VM state in memory and runs a real `dot-wire` guest.
    #[derive(Default)]
    pub struct FakeHv {
        pub status: StdMutex<HashMap<std::path::PathBuf, String>>,
        pub calls: StdMutex<Vec<String>>,
        pub guests: StdMutex<HashMap<std::path::PathBuf, tokio::task::JoinHandle<()>>>,
        pub secret: StdMutex<Vec<u8>>,
        pub fail_launch: StdMutex<bool>,
        /// Launch without a guest agent (a builder Mac in the installer).
        pub no_agent: StdMutex<bool>,
        /// The power button does nothing (a halt that freezes).
        pub ignore_powerdown: StdMutex<bool>,
        /// When set, the guest has been spinning in the kernel since then.
        pub frozen_since: StdMutex<Option<std::time::Instant>>,
    }

    impl FakeHv {
        fn log(&self, s: &str) {
            self.calls.lock().unwrap().push(s.to_owned());
        }
        fn set(&self, f: &qemu::Files, s: Option<&str>) {
            let mut map = self.status.lock().unwrap();
            match s {
                Some(s) => map.insert(f.dir.clone(), s.to_owned()),
                None => map.remove(&f.dir),
            };
        }
        /// Simulate one VM's QEMU dying.
        pub fn crash(&self, f: &qemu::Files) {
            self.set(f, None);
            if let Some(g) = self.guests.lock().unwrap().remove(&f.dir) {
                g.abort();
            }
        }
    }

    async fn serve_guest(listener: tokio::net::TcpListener, id: String, secret: Vec<u8>) {
        let mut conns = tokio::task::JoinSet::new();
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            let id = id.clone();
            let secret = secret.clone();
            conns.spawn(async move {
                if dot_wire::handshake::guest(&mut s, &secret, &id, "test", GuestInfo::default)
                    .await
                    .is_err()
                {
                    return;
                }
                let (mut r, mut w) = tokio::io::split(s);
                while let Ok(dot_wire::Envelope::Call { id, .. }) =
                    dot_wire::frame::read(&mut r).await
                {
                    let reply = dot_wire::Reply {
                        id,
                        result: Ok(serde_json::json!({})),
                    };
                    if dot_wire::frame::write(&mut w, &reply).await.is_err() {
                        return;
                    }
                }
            });
        }
    }

    fn tag(crypt: &Crypt) -> &'static str {
        match crypt {
            Crypt::Plain => "",
            Crypt::Key(_) => " encrypted",
            Crypt::Locked => " locked",
        }
    }

    impl Hypervisor for FakeHv {
        async fn provision(
            &self,
            _: &qemu::Files,
            _: u32,
            _: bool,
            crypt: &Crypt,
        ) -> anyhow::Result<()> {
            crypt.key()?;
            self.log(&format!("provision{}", tag(crypt)));
            Ok(())
        }
        async fn launch(
            &self,
            files: &qemu::Files,
            spec: &qemu::Spec,
            mode: LaunchMode,
            crypt: &Crypt,
        ) -> anyhow::Result<()> {
            crypt.key()?;
            self.log(&format!(
                "{}{}",
                if mode == LaunchMode::Resume {
                    "resume-launch"
                } else {
                    "launch"
                },
                tag(crypt)
            ));
            if *self.fail_launch.lock().unwrap() {
                anyhow::bail!("KVM unavailable");
            }
            assert!(
                files.config_img().exists(),
                "the config disk is written before launch"
            );
            self.set(files, Some("running"));
            if *self.no_agent.lock().unwrap() {
                return Ok(());
            }
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", spec.port)).await?;
            let secret = self.secret.lock().unwrap().clone();
            let handle = tokio::spawn(serve_guest(listener, spec.id.clone(), secret));
            self.guests
                .lock()
                .unwrap()
                .insert(files.dir.clone(), handle);
            Ok(())
        }
        async fn status(&self, f: &qemu::Files) -> Option<String> {
            self.status.lock().unwrap().get(&f.dir).cloned()
        }
        async fn activity(&self, f: &qemu::Files) -> Option<Activity> {
            self.status.lock().unwrap().get(&f.dir)?;
            let since = (*self.frozen_since.lock().unwrap())?;
            Some(Activity {
                io_ops: 7,
                cpu_seconds: since.elapsed().as_secs_f64(),
                rips: vec![0xffff_ff80_1234_5678, 0xffff_ff80_1234_5600],
            })
        }
        async fn powerdown(&self, f: &qemu::Files) -> anyhow::Result<()> {
            self.log("powerdown");
            if !*self.ignore_powerdown.lock().unwrap() {
                self.crash(f);
            }
            Ok(())
        }
        async fn kill(&self, f: &qemu::Files) -> anyhow::Result<()> {
            self.log("kill");
            self.crash(f);
            Ok(())
        }
        async fn reset(&self, _: &qemu::Files) -> anyhow::Result<()> {
            self.log("reset");
            *self.frozen_since.lock().unwrap() = None;
            Ok(())
        }
        async fn pause(&self, f: &qemu::Files) -> anyhow::Result<()> {
            self.log("pause");
            self.set(f, Some("paused"));
            Ok(())
        }
        async fn resume(&self, f: &qemu::Files) -> anyhow::Result<()> {
            self.log("resume");
            if self.status.lock().unwrap().contains_key(&f.dir) {
                self.set(f, Some("running"));
            }
            Ok(())
        }
        async fn hibernate(&self, files: &qemu::Files, crypt: &Crypt) -> anyhow::Result<()> {
            crypt.key()?;
            self.log(&format!("hibernate{}", tag(crypt)));
            std::fs::write(files.hibernate(), b"ram")?;
            self.crash(files);
            Ok(())
        }
        async fn snapshot(
            &self,
            _: &qemu::Files,
            name: &str,
            live: bool,
            crypt: &Crypt,
        ) -> anyhow::Result<()> {
            crypt.key()?;
            self.log(&format!("snapshot {name} live={live}{}", tag(crypt)));
            Ok(())
        }
        async fn restore(
            &self,
            _: &qemu::Files,
            name: &str,
            _: bool,
            crypt: &Crypt,
            _: &qemu::Spec,
        ) -> anyhow::Result<()> {
            crypt.key()?;
            self.log(&format!("restore {name}{}", tag(crypt)));
            Ok(())
        }
        async fn delete_snapshot(
            &self,
            _: &qemu::Files,
            name: &str,
            _: bool,
            crypt: &Crypt,
        ) -> anyhow::Result<()> {
            self.log(&format!("delete {name}{}", tag(crypt)));
            Ok(())
        }
        async fn encrypt(
            &self,
            _: &qemu::Files,
            _: &MacKey,
            progress: &(dyn Fn(f64) + Send + Sync),
        ) -> anyhow::Result<()> {
            self.log("encrypt");
            progress(0.5);
            Ok(())
        }
        async fn disk_bytes(&self, _: &qemu::Files) -> Option<u64> {
            Some(1)
        }
        async fn snapshot_state_bytes(&self, _: &qemu::Files) -> Option<u64> {
            Some(0)
        }
        async fn snapshot_sizes(&self, _: &qemu::Files) -> Option<HashMap<String, u64>> {
            Some(HashMap::new())
        }
    }

    pub async fn insert_instance(
        db: &Db,
        key: &MasterKey,
        id: &str,
        port: u16,
        memory_mb: u32,
    ) -> Vec<u8> {
        let secret = b"guest-secret".to_vec();
        let sealed = key.seal(&format!("guest:{id}"), &secret);
        let id = id.to_owned();
        db.call(move |c| {
            c.execute(
                "INSERT OR IGNORE INTO users (id, username, display_name, password_hash, role, created_at) VALUES ('u1','alex','Alex','x','admin',0)",
                [],
            )?;
            c.execute(
                "INSERT INTO instances (id, user_id, name, driver, cpus, memory_mb, disk_gb, port, secret, created_at)
                 VALUES (?1, 'u1', 'Studio', 'qemu', 2, ?3, 64, ?2, ?4, 0)",
                rusqlite::params![id, port, memory_mb, sealed],
            )
        })
        .await
        .unwrap();
        secret
    }

    pub fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn fleet(
        dir: &std::path::Path,
        memory_budget_mb: u64,
        max_running: u32,
    ) -> (Fleet, Db, MasterKey) {
        fleet_on(dir, Db::memory().unwrap(), memory_budget_mb, max_running)
    }

    fn fleet_with(
        dir: &std::path::Path,
        memory_budget_mb: u64,
        max_running: u32,
        tweak: impl FnOnce(&mut Config),
    ) -> (Fleet, Db, MasterKey) {
        fleet_on_with(
            dir,
            Db::memory().unwrap(),
            memory_budget_mb,
            max_running,
            tweak,
        )
    }

    /// A fleet over an existing database: a new one over the same database is what a
    /// server restart looks like (no keys in memory, no QEMU it knows of).
    fn fleet_on(
        dir: &std::path::Path,
        db: Db,
        memory_budget_mb: u64,
        max_running: u32,
    ) -> (Fleet, Db, MasterKey) {
        fleet_on_with(dir, db, memory_budget_mb, max_running, |_| {})
    }

    fn fleet_on_with(
        dir: &std::path::Path,
        db: Db,
        memory_budget_mb: u64,
        max_running: u32,
        tweak: impl FnOnce(&mut Config),
    ) -> (Fleet, Db, MasterKey) {
        let mut config: Config = toml::from_str("public_url = 'http://localhost:8470'").unwrap();
        config.data_dir = dir.to_path_buf();
        config.limits.memory_budget_mb = memory_budget_mb;
        config.limits.max_running = max_running;
        config.limits.min_free_disk_gb = 0;
        // macOS has no 127.77.0.1 loopback alias; the fake VMs listen on 127.0.0.1.
        config.vm.forward_address = std::net::Ipv4Addr::LOCALHOST;
        tweak(&mut config);
        let key = MasterKey::from_bytes(&[1; 32]).unwrap();
        let fake = FakeHv::default();
        *fake.secret.lock().unwrap() = b"guest-secret".to_vec();
        let fleet = Fleet::new(
            Arc::new(config),
            db.clone(),
            key.clone(),
            Events::default(),
            Hv::Fake(fake),
        );
        (fleet, db, key)
    }

    fn fake(fleet: &Fleet) -> &FakeHv {
        match &*fleet.inner.hv {
            Hv::Fake(f) => f,
            Hv::Qemu(_) => unreachable!(),
        }
    }

    async fn wait_for(m: &Machine, state: State) {
        let mut rx = m.watch();
        tokio::time::timeout(Duration::from_secs(20), async {
            while rx.borrow_and_update().state != state {
                rx.changed().await.unwrap();
            }
        })
        .await
        .unwrap_or_else(|_| panic!("never reached {state:?}; at {:?}", m.status()));
    }

    #[tokio::test]
    async fn boots_on_demand_sleeps_wakes_snapshots_and_stops() {
        let dir = tempfile::tempdir().unwrap();
        let (fleet, db, key) = fleet(dir.path(), 16_384, 4);
        let port = free_port();
        insert_instance(&db, &key, "m1", port, 4096).await;
        let m = fleet.machine("m1");
        wait_for(&m, State::Stopped).await;

        // A tool call to a stopped Mac starts it and waits for the agent.
        let link = m.ensure_ready(Duration::from_secs(20)).await.unwrap();
        assert!(link.is_alive());
        assert_eq!(m.status().state, State::Ready);

        m.sleep().await.unwrap();
        assert_eq!(m.status().state, State::Sleeping);
        assert!(m.link().is_none());
        m.ensure_ready(Duration::from_secs(20)).await.unwrap();

        assert!(
            m.snapshot("s1".into()).await.unwrap(),
            "running Macs get live snapshots"
        );

        m.hibernate().await.unwrap();
        assert_eq!(m.status().state, State::Hibernated);
        m.ensure_ready(Duration::from_secs(20)).await.unwrap();
        assert!(
            fake(&fleet)
                .calls
                .lock()
                .unwrap()
                .contains(&"resume-launch".to_owned())
        );

        m.stop(false).await.unwrap();
        assert_eq!(m.status().state, State::Stopped);
        let desired: String = db
            .call(|c| {
                c.query_row("SELECT desired FROM instances WHERE id='m1'", [], |r| {
                    r.get(0)
                })
            })
            .await
            .unwrap();
        assert_eq!(desired, "stopped");
    }

    #[tokio::test(start_paused = false)]
    async fn crash_is_restarted_and_budget_hibernates_idle_macs() {
        let dir = tempfile::tempdir().unwrap();
        let (fleet, db, key) = fleet(dir.path(), 6_000, 4);
        insert_instance(&db, &key, "a", free_port(), 4096).await;
        insert_instance(&db, &key, "b", free_port(), 4096).await;
        let a = fleet.machine("a");
        a.ensure_ready(Duration::from_secs(20)).await.unwrap();

        // Only room for one; "a" is busy (recent activity), so "b" is refused.
        let b = fleet.machine("b");
        let err = b.ensure_ready(Duration::from_secs(5)).await.unwrap_err();
        assert!(err.to_string().contains("limit"), "{err}");

        // Once "a" has been idle long enough, starting "b" hibernates it.
        a.last_activity.store(now() - 600, Ordering::Relaxed);
        b.ensure_ready(Duration::from_secs(20)).await.unwrap();
        assert_eq!(a.status().state, State::Hibernated);

        // QEMU dying unexpectedly is noticed and restarted.
        fake(&fleet).crash(&fleet.files("b"));
        wait_for(&b, State::Starting).await;
    }

    #[tokio::test]
    async fn launch_failure_is_reported_in_plain_words() {
        let dir = tempfile::tempdir().unwrap();
        let (fleet, db, key) = fleet(dir.path(), 16_384, 4);
        insert_instance(&db, &key, "m", free_port(), 2048).await;
        *fake(&fleet).fail_launch.lock().unwrap() = true;
        let m = fleet.machine("m");
        let err = m.ensure_ready(Duration::from_secs(5)).await.unwrap_err();
        assert!(err.to_string().contains("KVM unavailable"), "{err}");
        assert_eq!(m.status().state, State::Error);
    }

    #[test]
    fn only_a_busy_kernel_with_no_disk_activity_counts_as_frozen() {
        let t0 = std::time::Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let sample = |io_ops: u64, cpu_seconds: f64, rip: u64| {
            Some(Activity {
                io_ops,
                cpu_seconds,
                rips: vec![rip, 0xffff_ff80_0000_1000],
            })
        };
        let kernel = 0xffff_ff80_1234_5678;
        let window = Duration::from_secs(60);

        let mut w = FreezeWatch::default();
        assert!(!w.observe(at(0), sample(5, 0.0, kernel), window));
        assert!(!w.observe(at(30), sample(5, 30.0, kernel), window));
        assert!(
            w.observe(at(60), sample(5, 60.0, kernel), window),
            "spinning, no I/O"
        );

        let mut w = FreezeWatch::default();
        w.observe(at(0), sample(5, 0.0, kernel), window);
        w.observe(at(30), sample(6, 30.0, kernel), window);
        assert!(
            !w.observe(at(60), sample(6, 60.0, kernel), window),
            "a disk write restarts the clock"
        );
        assert!(w.observe(at(90), sample(6, 90.0, kernel), window));

        let mut w = FreezeWatch::default();
        w.observe(at(0), sample(5, 0.0, kernel), window);
        assert!(
            !w.observe(at(60), sample(5, 3.0, kernel), window),
            "idle, not spinning"
        );

        let mut w = FreezeWatch::default();
        w.observe(at(0), sample(5, 0.0, 0x7fe1_a2b4), window);
        assert!(
            !w.observe(at(60), sample(5, 60.0, 0x7fe1_a2b4), window),
            "OpenCore's picker spins too"
        );

        let mut w = FreezeWatch::default();
        w.observe(at(0), sample(5, 0.0, kernel), window);
        assert!(!w.observe(at(30), None, window));
        assert!(
            !w.observe(at(60), sample(5, 60.0, kernel), window),
            "a missed sample starts over"
        );
    }

    #[tokio::test]
    async fn a_restart_that_froze_is_reset() {
        let dir = tempfile::tempdir().unwrap();
        let (fleet, db, key) = fleet_with(dir.path(), 16_384, 4, |c| c.vm.frozen_reset_seconds = 2);
        insert_instance(&db, &key, "m", free_port(), 2048).await;
        // A builder in the installer: no agent ever answers.
        *fake(&fleet).no_agent.lock().unwrap() = true;
        let m = fleet.machine("m");
        wait_for(&m, State::Stopped).await;
        m.start().await.unwrap();
        assert_eq!(m.status().state, State::Booting);
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(
            !fake(&fleet)
                .calls
                .lock()
                .unwrap()
                .contains(&"reset".to_owned()),
            "a guest that isn't frozen is left alone"
        );

        *fake(&fleet).frozen_since.lock().unwrap() = Some(std::time::Instant::now());
        tokio::time::timeout(Duration::from_secs(15), async {
            while !fake(&fleet)
                .calls
                .lock()
                .unwrap()
                .contains(&"reset".to_owned())
            {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        })
        .await
        .expect("the frozen Mac was reset");
        assert_eq!(m.status().state, State::Booting);
        assert!(m.status().detail.unwrap_or_default().contains("froze"));
    }

    #[tokio::test]
    async fn a_shutdown_that_froze_is_turned_off_without_the_full_wait() {
        let dir = tempfile::tempdir().unwrap();
        let (fleet, db, key) = fleet_with(dir.path(), 16_384, 4, |c| c.vm.frozen_reset_seconds = 2);
        insert_instance(&db, &key, "m", free_port(), 2048).await;
        let m = fleet.machine("m");
        m.ensure_ready(Duration::from_secs(20)).await.unwrap();
        *fake(&fleet).ignore_powerdown.lock().unwrap() = true;
        *fake(&fleet).frozen_since.lock().unwrap() = Some(std::time::Instant::now());
        let started = std::time::Instant::now();
        m.stop(false).await.unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(m.status().state, State::Stopped);
        let calls = fake(&fleet).calls.lock().unwrap().clone();
        assert!(
            calls.ends_with(&["powerdown".to_owned(), "kill".to_owned()]),
            "{calls:?}"
        );
    }

    async fn encrypt_instance(db: &Db, id: &str) {
        let id = id.to_owned();
        db.call(move |c| c.execute("UPDATE instances SET encrypted = 1 WHERE id = ?1", [id]))
            .await
            .unwrap();
    }

    fn calls(fleet: &Fleet) -> Vec<String> {
        fake(fleet).calls.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn encrypted_macs_need_their_owners_key_and_lock_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let (fleet, db, key) = fleet(dir.path(), 16_384, 4);
        insert_instance(&db, &key, "m1", free_port(), 4096).await;
        encrypt_instance(&db, "m1").await;
        let data_key = crate::keys::DataKey::random();

        // Locked from the start: nothing launches, and the reason is plain.
        let m = fleet.machine("m1");
        wait_for(&m, State::Stopped).await;
        for _ in 0..50 {
            if m.is_locked() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(m.is_locked());
        assert!(matches!(
            m.ensure_ready(Duration::from_secs(5)).await,
            Err(ReadyError::Locked)
        ));
        assert_eq!(m.start().await.unwrap_err(), LOCKED);
        assert_eq!(m.status().detail.as_deref(), Some(LOCKED_DETAIL));
        assert!(!calls(&fleet).iter().any(|c| c.starts_with("launch")));

        // Signing in unlocks it; the Mac that was asked to start starts by itself.
        fleet.keys().insert("u1", data_key.clone());
        fleet.unlocked("u1").await;
        wait_for(&m, State::Ready).await;
        assert!(!m.is_locked());
        let log = calls(&fleet);
        assert!(log.contains(&"provision encrypted".to_owned()), "{log:?}");
        assert!(log.contains(&"launch encrypted".to_owned()), "{log:?}");
        m.snapshot("live".into()).await.unwrap();
        assert!(calls(&fleet).contains(&"snapshot live live=true encrypted".to_owned()));

        // Saved memory needs the key too.
        m.hibernate().await.unwrap();
        assert!(calls(&fleet).contains(&"hibernate encrypted".to_owned()));
        assert_eq!(m.status().state, State::Hibernated);

        // A restart: same database, nothing in memory. The hibernated Mac waits.
        fleet.shutdown(true).await;
        let (after, _, _) = fleet_on(dir.path(), db.clone(), 16_384, 4);
        *fake(&after).secret.lock().unwrap() = b"guest-secret".to_vec();
        let m = after.machine("m1");
        wait_for(&m, State::Hibernated).await;
        assert_eq!(m.status().detail.as_deref(), Some(LOCKED_DETAIL));
        assert!(matches!(
            m.ensure_ready(Duration::from_secs(5)).await,
            Err(ReadyError::Locked)
        ));
        m.wake().await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(m.status().state, State::Hibernated);
        assert!(
            !calls(&after).iter().any(|c| c.starts_with("resume-launch")),
            "never resumed without the key"
        );
        assert_eq!(
            m.snapshot("x".into()).await.unwrap_err(),
            "Wake the Mac or shut it down before taking a snapshot."
        );

        after.keys().insert("u1", data_key);
        after.unlocked("u1").await;
        let mut rx = m.watch();
        tokio::time::timeout(Duration::from_secs(5), async {
            while rx.borrow_and_update().detail.as_deref() == Some(LOCKED_DETAIL) {
                rx.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!(
            m.status().state,
            State::Hibernated,
            "wakes on demand, not by itself"
        );
        m.ensure_ready(Duration::from_secs(20)).await.unwrap();
        assert!(calls(&after).contains(&"resume-launch encrypted".to_owned()));

        // Clearing the keys again (another restart) leaves a running Mac running, but
        // it can't save its memory or be restarted from off.
        after.keys().clear();
        assert!(m.is_locked());
        assert_eq!(m.hibernate().await.unwrap_err(), LOCKED);
        assert_eq!(m.status().state, State::Ready);
        assert_eq!(m.snapshot("y".into()).await.unwrap_err(), LOCKED);
        m.stop(true).await.unwrap();
        assert_eq!(m.start().await.unwrap_err(), LOCKED);
    }

    #[tokio::test]
    async fn plain_macs_encrypt_when_stopped_and_unlocked() {
        let dir = tempfile::tempdir().unwrap();
        let (fleet, db, key) = fleet(dir.path(), 16_384, 4);
        insert_instance(&db, &key, "old", free_port(), 4096).await;
        db.call(|c| {
            c.execute(
                "INSERT INTO snapshots (id, instance_id, name, tag, live, created_by, created_at) VALUES ('s1', 'old', 'Before', 'dot-s1', 1, 'owner', 0)",
                [],
            )
        })
        .await
        .unwrap();
        let m = fleet.machine("old");
        m.ensure_ready(Duration::from_secs(20)).await.unwrap();
        assert!(
            calls(&fleet).contains(&"launch".to_owned()),
            "plain Macs need no key"
        );
        assert!(!m.is_locked());

        assert_eq!(m.encrypt().await.unwrap_err(), "Shut down this Mac first.");
        m.stop(false).await.unwrap();
        assert_eq!(m.encrypt().await.unwrap_err(), LOCKED);

        fleet.keys().insert("u1", crate::keys::DataKey::random());
        std::fs::write(fleet.files("old").disk(), b"disk").unwrap();
        m.encrypt().await.unwrap();
        assert_eq!(m.start().await.unwrap_err(), ENCRYPTING);
        wait_for(&m, State::Stopped).await;
        assert!(calls(&fleet).contains(&"encrypt".to_owned()));
        let (encrypted, snapshots): (bool, i64) = db
            .call(|c| {
                c.query_row(
                    "SELECT encrypted, (SELECT COUNT(*) FROM snapshots WHERE instance_id = 'old') FROM instances WHERE id = 'old'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
            })
            .await
            .unwrap();
        assert!(encrypted);
        assert_eq!(snapshots, 0, "converting drops internal snapshots");
        assert_eq!(
            m.encrypt().await.unwrap_err(),
            "This Mac is already encrypted."
        );
        m.ensure_ready(Duration::from_secs(20)).await.unwrap();
        assert!(calls(&fleet).contains(&"launch encrypted".to_owned()));
    }
}
