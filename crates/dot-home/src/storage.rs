//! What each person's Macs keep on the server's disk, and the limits on it
//! (docs/home/API.md "Storage").
//!
//! Sizes are bytes actually allocated on disk: a Mac's qcow2 overlay is sparse, so its
//! `disk_gb` is a ceiling, not a cost. Live snapshots live inside the overlay; their
//! saved memory (`vm-state-size`) is counted as snapshots and taken out of the disk
//! figure so nothing is counted twice.
use crate::{
    db::{now, rfc3339},
    error::{ApiError, ApiResult},
    state::App,
    vm::qemu::Files,
};
use rusqlite::OptionalExtension;
use serde::Serialize;
use std::{
    collections::HashMap,
    os::unix::fs::MetadataExt,
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};

/// Storage limits are in decimal gigabytes, like Finder and the portal show them.
pub const GB: u64 = 1_000_000_000;

/// Measurements are reused for this long.
const FRESH: Duration = Duration::from_secs(60);

/// Settings keys for the server defaults.
const QUOTA_KEY: &str = "storage_quota_gb";
const MAX_DISK_KEY: &str = "storage_max_disk_gb";
const MAX_SNAPSHOTS_KEY: &str = "storage_max_snapshots";
const DEFAULT_DISK_KEY: &str = "default_disk_gb";
const SNAPSHOT_TOTAL_KEY: &str = "storage_snapshot_total_gb";
/// Shared with the supervisor, which refuses to start Macs below it.
pub const RESERVE_KEY: &str = "reserve_gb";

/// `StorageLimits`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Limits {
    /// Everything a person's Macs may use together. 0 = no limit.
    pub quota_gb: u64,
    /// Largest disk one new Mac may have.
    pub max_disk_gb: u64,
    /// Snapshots per Mac. 0 = no limit.
    pub max_snapshots: u64,
}

impl Limits {
    /// At or over the quota (never, when the quota is 0).
    pub fn over_quota(&self, used_bytes: u64) -> bool {
        self.quota_gb > 0 && used_bytes >= self.quota_gb.saturating_mul(GB)
    }
    /// The Mac already has as many snapshots as allowed.
    pub fn snapshots_full(&self, count: u64) -> bool {
        self.max_snapshots > 0 && count >= self.max_snapshots
    }
}

/// `StorageDefaults`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Defaults {
    /// Limits for everyone without an override.
    #[serde(flatten)]
    pub limits: Limits,
    /// Disk size a new Mac gets unless its owner picks another.
    pub default_disk_gb: u64,
    /// New Macs and snapshots stop below this much free space.
    pub reserve_gb: u64,
    /// Most space all snapshots on the server may use together; past it the oldest
    /// unkept ones are deleted. 0 = no limit.
    pub snapshot_total_gb: u64,
}

/// `StorageOverrides`: `None` uses the server default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Overrides {
    /// Quota.
    pub quota_gb: Option<u64>,
    /// Largest disk.
    pub max_disk_gb: Option<u64>,
    /// Snapshots per Mac.
    pub max_snapshots: Option<u64>,
}

/// Where each effective limit came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Sources {
    quota_gb: &'static str,
    max_disk_gb: &'static str,
    max_snapshots: &'static str,
}

impl Overrides {
    /// The person's effective limits.
    pub fn resolve(&self, defaults: &Limits) -> (Limits, Sources) {
        let source = |o: Option<u64>| if o.is_some() { "override" } else { "default" };
        (
            Limits {
                quota_gb: self.quota_gb.unwrap_or(defaults.quota_gb),
                max_disk_gb: self.max_disk_gb.unwrap_or(defaults.max_disk_gb),
                max_snapshots: self.max_snapshots.unwrap_or(defaults.max_snapshots),
            },
            Sources {
                quota_gb: source(self.quota_gb),
                max_disk_gb: source(self.max_disk_gb),
                max_snapshots: source(self.max_snapshots),
            },
        )
    }
}

async fn number(app: &App, key: &'static str) -> ApiResult<Option<u64>> {
    Ok(app.db.setting(key).await?.and_then(|v| v.parse().ok()))
}

/// Server defaults (documented fallbacks when never set).
pub async fn defaults(app: &App) -> ApiResult<Defaults> {
    Ok(Defaults {
        limits: Limits {
            quota_gb: number(app, QUOTA_KEY).await?.unwrap_or(0),
            max_disk_gb: number(app, MAX_DISK_KEY).await?.unwrap_or(512),
            max_snapshots: number(app, MAX_SNAPSHOTS_KEY).await?.unwrap_or(0),
        },
        default_disk_gb: number(app, DEFAULT_DISK_KEY)
            .await?
            .unwrap_or_else(|| u64::from(app.config.vm.default_disk_gb)),
        reserve_gb: app.fleet.reserve_gb().await,
        snapshot_total_gb: snapshot_total_gb(app).await,
    })
}

/// The server-wide snapshot cap in GB (0 = no limit).
pub async fn snapshot_total_gb(app: &App) -> u64 {
    number(app, SNAPSHOT_TOTAL_KEY)
        .await
        .ok()
        .flatten()
        .unwrap_or(0)
}

/// Save the defaults that are `Some`.
pub async fn set_defaults(
    app: &App,
    quota_gb: Option<u64>,
    max_disk_gb: Option<u64>,
    max_snapshots: Option<u64>,
    default_disk_gb: Option<u64>,
    reserve_gb: Option<u64>,
    snapshot_total_gb: Option<u64>,
) -> ApiResult<()> {
    for (key, value) in [
        (QUOTA_KEY, quota_gb),
        (MAX_DISK_KEY, max_disk_gb),
        (MAX_SNAPSHOTS_KEY, max_snapshots),
        (DEFAULT_DISK_KEY, default_disk_gb),
        (RESERVE_KEY, reserve_gb),
        (SNAPSHOT_TOTAL_KEY, snapshot_total_gb),
    ] {
        if let Some(v) = value {
            app.db.set_setting(key, v.to_string()).await?;
        }
    }
    Ok(())
}

const OVERRIDE_COLUMNS: &str = "storage_quota_gb, max_disk_gb, max_snapshots";

fn overrides_from_row(r: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<Overrides> {
    Ok(Overrides {
        quota_gb: r.get(at)?,
        max_disk_gb: r.get(at + 1)?,
        max_snapshots: r.get(at + 2)?,
    })
}

/// A person's overrides; `None` when they don't exist.
pub async fn overrides(app: &App, user_id: &str) -> ApiResult<Option<Overrides>> {
    let uid = user_id.to_owned();
    Ok(app
        .db
        .call(move |c| {
            c.query_row(
                &format!("SELECT {OVERRIDE_COLUMNS} FROM users WHERE id = ?1"),
                [uid],
                |r| overrides_from_row(r, 0),
            )
            .optional()
        })
        .await?)
}

/// A person's effective limits and where they came from.
pub async fn limits_for(app: &App, user_id: &str) -> ApiResult<(Limits, Sources)> {
    let d = defaults(app).await?;
    Ok(overrides(app, user_id)
        .await?
        .unwrap_or_default()
        .resolve(&d.limits))
}

// ---------------------------------------------------------------------------
// Measuring.

/// Bytes one Mac keeps on the server.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MacUsage {
    /// The Mac's own disk overlay, firmware variables and config disk.
    pub disk_bytes: u64,
    /// Memory saved inside live snapshots.
    pub snapshots_bytes: u64,
    /// Memory saved while hibernated.
    pub memory_bytes: u64,
}

impl MacUsage {
    /// Everything together.
    pub fn total(&self) -> u64 {
        self.disk_bytes + self.snapshots_bytes + self.memory_bytes
    }
}

/// Bytes allocated for one file (only what was written, for sparse files). 0 when missing.
pub fn allocated(path: &Path) -> u64 {
    std::fs::symlink_metadata(path).map_or(0, |m| m.blocks() * 512)
}

/// Bytes allocated for everything under `dir`, without following symlinks.
pub fn tree_allocated(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return allocated(dir);
    };
    entries
        .filter_map(Result::ok)
        .map(|e| {
            let path = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => tree_allocated(&path),
                _ => allocated(&path),
            }
        })
        .sum()
}

/// Split one instance directory into disk, snapshots and saved memory. `snapshot_state`
/// is the memory held by live snapshots inside `disk.qcow2`; encrypted Macs keep it in
/// files of their own instead (`snapshots/`).
pub fn measure_files(files: &Files, snapshot_state: u64) -> MacUsage {
    let everything = tree_allocated(&files.dir);
    let memory = allocated(&files.hibernate());
    let snapshots =
        snapshot_state.min(allocated(&files.disk())) + tree_allocated(&files.snapshot_memory_dir());
    MacUsage {
        disk_bytes: everything.saturating_sub(memory + snapshots),
        snapshots_bytes: snapshots,
        memory_bytes: memory,
    }
}

/// Recent measurements per Mac.
#[derive(Default)]
pub struct Cache {
    entries: Mutex<HashMap<String, Measured>>,
}

#[derive(Debug, Clone, Copy)]
struct Measured {
    taken: Instant,
    at: i64,
    usage: MacUsage,
}

impl Cache {
    fn fresh(&self, id: &str) -> Option<Measured> {
        self.lock()
            .get(id)
            .copied()
            .filter(|m| m.taken.elapsed() < FRESH)
    }
    fn put(&self, id: &str, m: Measured) {
        self.lock().insert(id.to_owned(), m);
    }
    /// Measure this Mac again next time (its files just changed).
    pub fn forget(&self, id: &str) {
        self.lock().remove(id);
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Measured>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// One Mac's usage (cached for a minute) and when it was measured (unix seconds).
async fn measure(app: &App, id: &str, local: bool) -> (MacUsage, i64) {
    if !local {
        return (MacUsage::default(), now());
    }
    if let Some(m) = app.storage.fresh(id) {
        return (m.usage, m.at);
    }
    let files = app.fleet.files(id);
    let state = app
        .fleet
        .hv()
        .snapshot_state_bytes(&files)
        .await
        .unwrap_or(0);
    let usage = tokio::task::spawn_blocking(move || measure_files(&files, state))
        .await
        .unwrap_or_default();
    let m = Measured {
        taken: Instant::now(),
        at: now(),
        usage,
    };
    app.storage.put(id, m);
    (usage, m.at)
}

/// `MacStorage`.
#[derive(Debug, Clone, Serialize)]
pub struct MacStorage {
    instance_id: String,
    name: String,
    driver: String,
    state: crate::vm::State,
    disk_gb: u64,
    used_bytes: u64,
    disk_bytes: u64,
    snapshots_bytes: u64,
    memory_bytes: u64,
    snapshot_count: u64,
}

/// One person's Macs, what they use together, and the oldest measurement time.
pub async fn usage(app: &App, user_id: &str) -> ApiResult<(Vec<MacStorage>, u64, i64)> {
    let uid = user_id.to_owned();
    let rows: Vec<(String, String, String, u64, u64)> = app
        .db
        .call(move |c| {
            let mut st = c.prepare(
                "SELECT i.id, i.name, i.driver, i.disk_gb, (SELECT COUNT(*) FROM snapshots s WHERE s.instance_id = i.id)
                 FROM instances i WHERE i.user_id = ?1 AND i.is_builder = 0 ORDER BY i.created_at",
            )?;
            let rows = st.query_map([uid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
            rows.collect()
        })
        .await?;
    let mut macs = Vec::with_capacity(rows.len());
    let mut used = 0;
    let mut oldest = now();
    for (id, name, driver, disk_gb, snapshot_count) in rows {
        let (u, at) = measure(app, &id, driver == "qemu").await;
        used += u.total();
        oldest = oldest.min(at);
        macs.push(MacStorage {
            state: app.fleet.machine(&id).status().state,
            instance_id: id,
            name,
            driver,
            disk_gb,
            used_bytes: u.total(),
            disk_bytes: u.disk_bytes,
            snapshots_bytes: u.snapshots_bytes,
            memory_bytes: u.memory_bytes,
            snapshot_count,
        });
    }
    Ok((macs, used, oldest))
}

/// `MyStorage`.
pub async fn mine(app: &App, user_id: &str) -> ApiResult<serde_json::Value> {
    let (limits, sources) = limits_for(app, user_id).await?;
    let (macs, used, at) = usage(app, user_id).await?;
    Ok(serde_json::json!({
        "limits": limits, "sources": sources, "used_bytes": used,
        "over_quota": limits.over_quota(used), "macs": macs, "measured_at": rfc3339(at)
    }))
}

/// `AdminStorage`.
pub async fn admin(app: &App) -> ApiResult<serde_json::Value> {
    let d = defaults(app).await?;
    let people: Vec<(String, String, String, String, Overrides)> = app
        .db
        .call(|c| {
            let mut st = c.prepare(&format!(
                "SELECT id, username, display_name, role, {OVERRIDE_COLUMNS} FROM users ORDER BY created_at"
            ))?;
            let rows = st.query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, overrides_from_row(r, 4)?))
            })?;
            rows.collect()
        })
        .await?;
    let mut out = Vec::with_capacity(people.len());
    let mut oldest = now();
    let mut committed = 0u64;
    let mut unlimited = false;
    for (id, username, display_name, role, o) in people {
        let (limits, _) = o.resolve(&d.limits);
        let (macs, used, at) = usage(app, &id).await?;
        oldest = oldest.min(at);
        unlimited |= limits.quota_gb == 0;
        committed = committed.saturating_add(limits.quota_gb);
        out.push(serde_json::json!({
            "user_id": id, "username": username, "display_name": display_name, "role": role,
            "used_bytes": used, "mac_count": macs.len(), "overrides": o, "limits": limits,
            "over_quota": limits.over_quota(used)
        }));
    }
    let data_dir = app.config.data_dir.clone();
    let (disk, images) = tokio::task::spawn_blocking(move || {
        (
            crate::host::disk(&data_dir).unwrap_or((0, 0)),
            tree_allocated(&data_dir.join("images")),
        )
    })
    .await
    .map_err(ApiError::internal)?;
    Ok(serde_json::json!({
        "disk_total_bytes": disk.0, "disk_free_bytes": disk.1, "images_bytes": images,
        "committed_gb": if unlimited { 0 } else { committed },
        "defaults": d, "people": out, "measured_at": rfc3339(oldest)
    }))
}

// ---------------------------------------------------------------------------
// Enforcement.

/// "12.3 GB".
fn gb_text(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / GB as f64)
}

const FREE_SPACE: &str = "Delete snapshots or a Mac you don't need, or ask your admin for more.";

/// Free space is under the reserve.
async fn below_reserve(app: &App) -> bool {
    let reserve = app.fleet.reserve_gb().await.saturating_mul(GB);
    let dir = app.config.data_dir.clone();
    tokio::task::spawn_blocking(move || crate::host::disk_free(&dir))
        .await
        .ok()
        .flatten()
        .is_some_and(|free| free < reserve)
}

/// Can this person add a Mac on this server with a `disk_gb` disk?
pub async fn check_new_mac(app: &App, user_id: &str, disk_gb: u64) -> ApiResult<()> {
    let (limits, _) = limits_for(app, user_id).await?;
    if disk_gb > limits.max_disk_gb {
        return Err(ApiError::invalid(
            "disk_gb",
            format!(
                "Macs here can have a disk of up to {} GB. Choose a smaller disk, or ask your admin for more.",
                limits.max_disk_gb
            ),
        ));
    }
    let (_, used, _) = usage(app, user_id).await?;
    if limits.over_quota(used) {
        return Err(ApiError::budget(format!(
            "Your Macs use {} of your {} GB of storage, so you can't add another. {FREE_SPACE}",
            gb_text(used),
            limits.quota_gb
        )));
    }
    if below_reserve(app).await {
        return Err(ApiError::budget(
            "The server is almost out of disk space, so new Macs can't be added. An admin needs to free space.",
        ));
    }
    Ok(())
}

/// Can this Mac take one more snapshot? The error is a sentence for the person or agent.
pub async fn check_snapshot(app: &App, instance_id: &str) -> Result<(), String> {
    let iid = instance_id.to_owned();
    let row: Option<(String, u64)> = app
        .db
        .call(move |c| {
            c.query_row(
                "SELECT user_id, (SELECT COUNT(*) FROM snapshots WHERE instance_id = ?1) FROM instances WHERE id = ?1",
                [iid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })
        .await
        .map_err(|e| e.to_string())?;
    let Some((owner, count)) = row else {
        return Err("That Mac doesn't exist.".into());
    };
    let (limits, _) = limits_for(app, &owner).await.map_err(|e| e.message)?;
    if limits.snapshots_full(count) {
        return Err(format!(
            "This Mac already has {count} snapshots, the most allowed. Delete one you don't need, then try again."
        ));
    }
    let (_, used, _) = usage(app, &owner).await.map_err(|e| e.message)?;
    if limits.over_quota(used) {
        return Err(format!(
            "Your Macs use {} of your {} GB of storage, so no snapshot was taken. {FREE_SPACE}",
            gb_text(used),
            limits.quota_gb
        ));
    }
    if below_reserve(app).await {
        return Err(
            "The server is almost out of disk space, so no snapshot was taken. An admin needs to free space."
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULTS: Limits = Limits {
        quota_gb: 200,
        max_disk_gb: 512,
        max_snapshots: 10,
    };

    #[test]
    fn overrides_win_over_defaults_one_limit_at_a_time() {
        let o = Overrides {
            quota_gb: Some(50),
            max_disk_gb: None,
            max_snapshots: Some(0),
        };
        let (limits, sources) = o.resolve(&DEFAULTS);
        assert_eq!(
            limits,
            Limits {
                quota_gb: 50,
                max_disk_gb: 512,
                max_snapshots: 0
            }
        );
        assert_eq!(
            (sources.quota_gb, sources.max_disk_gb, sources.max_snapshots),
            ("override", "default", "override")
        );
        assert_eq!(Overrides::default().resolve(&DEFAULTS).0, DEFAULTS);
    }

    #[test]
    fn zero_means_no_limit() {
        let unlimited = Limits {
            quota_gb: 0,
            max_disk_gb: 512,
            max_snapshots: 0,
        };
        assert!(!unlimited.over_quota(u64::MAX));
        assert!(!unlimited.snapshots_full(1_000));
        assert!(DEFAULTS.over_quota(200 * GB), "at the quota counts as over");
        assert!(!DEFAULTS.over_quota(200 * GB - 1));
        assert!(DEFAULTS.snapshots_full(10));
        assert!(!DEFAULTS.snapshots_full(9));
    }

    #[test]
    fn measures_allocated_bytes_and_splits_memory_and_snapshots_out_of_the_disk() {
        use std::io::{Seek, SeekFrom, Write};
        let dir = tempfile::tempdir().unwrap();
        let files = Files::new(dir.path(), "m1");
        std::fs::create_dir_all(&files.dir).unwrap();
        // A sparse 10 GB disk with 1 MiB actually written costs about 1 MiB.
        let mut disk = std::fs::File::create(files.disk()).unwrap();
        disk.write_all(&vec![7u8; 1 << 20]).unwrap();
        disk.seek(SeekFrom::Start(10 * GB)).unwrap();
        disk.write_all(b"end").unwrap();
        disk.sync_all().unwrap();
        std::fs::write(files.hibernate(), vec![1u8; 256 * 1024]).unwrap();

        let written = allocated(&files.disk());
        assert!((1 << 20..64 << 20).contains(&written), "sparse: {written}");
        let memory = allocated(&files.hibernate());
        assert!(memory >= 256 * 1024);

        let u = measure_files(&files, 300 * 1024);
        assert_eq!(u.memory_bytes, memory);
        assert_eq!(u.snapshots_bytes, 300 * 1024);
        assert_eq!(
            u.total(),
            tree_allocated(&files.dir),
            "nothing counted twice"
        );

        // Snapshot state can't be bigger than the disk file that holds it.
        assert_eq!(measure_files(&files, u64::MAX).snapshots_bytes, written);

        // An encrypted Mac's live snapshot memory sits next to the disk.
        std::fs::create_dir_all(files.snapshot_memory_dir()).unwrap();
        std::fs::write(files.snapshot_memory("dot-a"), vec![2u8; 128 * 1024]).unwrap();
        let saved = tree_allocated(&files.snapshot_memory_dir());
        let u = measure_files(&files, 0);
        assert_eq!(u.snapshots_bytes, saved);
        assert_eq!(u.total(), tree_allocated(&files.dir));
        assert_eq!(
            measure_files(&Files::new(dir.path(), "missing"), 0),
            MacUsage::default()
        );
    }
}
