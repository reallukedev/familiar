//! Checkpoints of a whole Mac, shared by the portal and the agent's tools, and the
//! sweeper that deletes old ones (docs/home/API.md "Snapshots").
use crate::{
    crypto,
    db::{now, rfc3339},
    state::App,
    storage::GB,
};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
};

/// Portal/agent view.
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotView {
    /// Id.
    pub id: String,
    /// Name.
    pub name: String,
    /// RFC 3339.
    pub created_at: String,
    /// Includes memory.
    pub live: bool,
    /// Memory saved with a live snapshot; disk-only snapshots share blocks with the
    /// disk and are 0. `None` when it couldn't be measured.
    pub size_bytes: Option<u64>,
    /// `owner` or `agent`.
    pub created_by: String,
    /// Marked to keep: never deleted automatically.
    pub kept: bool,
    /// When the retention sweep deletes it (RFC 3339); `None` when kept or kept forever.
    pub expires_at: Option<String>,
}

const KEEP_AGENT: usize = 10;

/// How often the sweeper runs after the one at startup.
const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);

/// Longest retention the API accepts, in days.
pub const MAX_KEEP_DAYS: i64 = 3650;

/// Why a checkpoint wasn't taken. Each carries a sentence for the person or agent.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CreateError {
    /// The name was empty or too long.
    #[error("{0}")]
    Invalid(String),
    /// A storage limit (quota, snapshots per Mac, server reserve) refused it.
    #[error("{0}")]
    Limit(String),
    /// The Mac couldn't take it.
    #[error("{0}")]
    Failed(String),
}

// ---------------------------------------------------------------------------
// Busy snapshots: being created or restored. The sweeper never deletes these.

static BUSY: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

fn busy() -> std::sync::MutexGuard<'static, HashSet<String>> {
    BUSY.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Holds a snapshot id busy until dropped.
struct Claim(String);

impl Claim {
    /// `None` when something else already holds it.
    fn take(id: &str) -> Option<Self> {
        busy().insert(id.to_owned()).then(|| Self(id.to_owned()))
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        busy().remove(&self.0);
    }
}

fn is_busy(id: &str) -> bool {
    busy().contains(id)
}

// ---------------------------------------------------------------------------
// Taking, listing, restoring, deleting.

/// Take a checkpoint, unless a storage limit says no.
pub async fn create(
    app: &App,
    instance_id: &str,
    name: &str,
    created_by: &'static str,
) -> Result<SnapshotView, CreateError> {
    let name = crate::util::clean_name(name, 60).ok_or_else(|| {
        CreateError::Invalid("Give the checkpoint a name of 1 to 60 characters.".into())
    })?;
    crate::storage::check_snapshot(app, instance_id)
        .await
        .map_err(CreateError::Limit)?;
    let id = crypto::random_id::<8>();
    // Busy from now until the caps below have run, so they never pick it.
    let _claim = Claim::take(&id);
    let tag = format!("dot-{id}");
    let machine = app.fleet.machine(instance_id);
    let live = machine
        .snapshot(tag.clone())
        .await
        .map_err(CreateError::Failed)?;
    app.storage.forget(instance_id);
    let (iid, id2, name2) = (instance_id.to_owned(), id.clone(), name.clone());
    let t = now();
    let keep_days: i64 = app
        .db
        .call(move |c| {
            c.execute(
                "INSERT INTO snapshots (id, instance_id, name, tag, live, created_by, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![id2, iid, name2, tag, live, created_by, t],
            )?;
            c.query_row(
                "SELECT snapshot_keep_days FROM instances WHERE id = ?1",
                [iid],
                |r| r.get(0),
            )
        })
        .await
        .map_err(|e| CreateError::Failed(e.to_string()))?;
    if created_by == "agent" {
        prune_agent(app, instance_id).await;
    }
    enforce_caps(app, Some(instance_id)).await;
    Ok(SnapshotView {
        id,
        name,
        created_at: rfc3339(t),
        live,
        size_bytes: None,
        created_by: created_by.to_owned(),
        kept: false,
        expires_at: expires_at(t, false, keep_days).map(rfc3339),
    })
}

/// The agent may checkpoint often; keep only its newest few (kept ones aside).
async fn prune_agent(app: &App, instance_id: &str) {
    let iid = instance_id.to_owned();
    let old: Vec<String> = app
        .db
        .call(move |c| {
            let mut s = c.prepare(
                "SELECT id FROM snapshots WHERE instance_id = ?1 AND created_by = 'agent' AND kept = 0 ORDER BY created_at DESC LIMIT -1 OFFSET ?2",
            )?;
            let rows = s.query_map(rusqlite::params![iid, KEEP_AGENT], |r| r.get(0))?;
            rows.collect()
        })
        .await
        .unwrap_or_default();
    for id in old {
        let Some(_claim) = Claim::take(&id) else {
            continue;
        };
        let _ = delete(app, instance_id, &id).await;
    }
}

/// Newest first.
pub async fn list(app: &App, instance_id: &str) -> anyhow::Result<Vec<SnapshotView>> {
    let iid = instance_id.to_owned();
    let rows: Vec<(SnapshotView, String, i64, i64)> = app
        .db
        .call(move |c| {
            let mut s = c.prepare(
                "SELECT s.id, s.name, s.created_at, s.live, s.created_by, s.kept, s.tag, i.snapshot_keep_days
                 FROM snapshots s JOIN instances i ON i.id = s.instance_id
                 WHERE s.instance_id = ?1 ORDER BY s.created_at DESC",
            )?;
            let rows = s.query_map([iid], |r| {
                let created: i64 = r.get(2)?;
                Ok((
                    SnapshotView {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        created_at: rfc3339(created),
                        live: r.get(3)?,
                        size_bytes: None,
                        created_by: r.get(4)?,
                        kept: r.get(5)?,
                        expires_at: None,
                    },
                    r.get(6)?,
                    created,
                    r.get(7)?,
                ))
            })?;
            rows.collect()
        })
        .await?;
    let sizes = if rows.is_empty() {
        None
    } else {
        app.fleet
            .hv()
            .snapshot_sizes(&app.fleet.files(instance_id))
            .await
    };
    Ok(rows
        .into_iter()
        .map(|(mut v, tag, created, keep_days)| {
            v.size_bytes = sizes.as_ref().map(|s| s.get(&tag).copied().unwrap_or(0));
            v.expires_at = expires_at(created, v.kept, keep_days).map(rfc3339);
            v
        })
        .collect())
}

async fn tag_of(app: &App, instance_id: &str, id: &str) -> Result<String, String> {
    let (iid, id) = (instance_id.to_owned(), id.to_owned());
    app.db
        .call(move |c| {
            c.query_row(
                "SELECT tag FROM snapshots WHERE id = ?1 AND instance_id = ?2",
                [id, iid],
                |r| r.get(0),
            )
        })
        .await
        .map_err(|_| "That checkpoint doesn't exist.".to_owned())
}

/// Roll back.
pub async fn restore(app: &App, instance_id: &str, id: &str) -> Result<(), String> {
    let tag = tag_of(app, instance_id, id).await?;
    let _claim = Claim::take(id)
        .ok_or_else(|| "That checkpoint is busy. Try again in a moment.".to_owned())?;
    app.fleet.machine(instance_id).restore(tag).await
}

/// Delete. The one path for manual deletes, agent pruning and the sweeper, so the
/// qcow2 tag, the row and the storage measurement stay consistent.
pub async fn delete(app: &App, instance_id: &str, id: &str) -> Result<(), String> {
    let tag = tag_of(app, instance_id, id).await?;
    // A missing qcow2 tag (already gone) shouldn't keep a dead row around.
    if let Err(e) = app.fleet.machine(instance_id).delete_snapshot(tag).await {
        tracing::warn!(instance = %instance_id, error = %e, "snapshot delete");
    }
    app.storage.forget(instance_id);
    let (iid, id) = (instance_id.to_owned(), id.to_owned());
    app.db
        .call(move |c| {
            c.execute(
                "DELETE FROM snapshots WHERE id = ?1 AND instance_id = ?2",
                [id, iid],
            )
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Mark a snapshot to keep (never deleted automatically) or not.
pub async fn set_kept(
    app: &App,
    instance_id: &str,
    id: &str,
    kept: bool,
) -> Result<SnapshotView, String> {
    let (iid, sid) = (instance_id.to_owned(), id.to_owned());
    let changed = app
        .db
        .call(move |c| {
            c.execute(
                "UPDATE snapshots SET kept = ?3 WHERE id = ?1 AND instance_id = ?2",
                rusqlite::params![sid, iid, kept],
            )
        })
        .await
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Err("That checkpoint doesn't exist.".into());
    }
    list(app, instance_id)
        .await
        .map_err(|e| format!("{e:#}"))?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| "That checkpoint doesn't exist.".into())
}

// ---------------------------------------------------------------------------
// Retention and caps: the rules, kept pure so they're easy to test.

/// One snapshot as the sweeper sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Snapshot id.
    pub id: String,
    /// Its Mac.
    pub instance_id: String,
    /// Unix seconds.
    pub created_at: i64,
    /// Marked to keep.
    pub kept: bool,
    /// Bytes it holds (see [`SnapshotView::size_bytes`]).
    pub size_bytes: u64,
}

/// When an unkept snapshot expires; `None` when kept or `keep_days` is 0 (forever).
pub fn expires_at(created_at: i64, kept: bool, keep_days: i64) -> Option<i64> {
    (!kept && keep_days > 0).then(|| created_at.saturating_add(keep_days.saturating_mul(86_400)))
}

/// Older than the Mac's retention and not kept.
pub fn is_expired(created_at: i64, kept: bool, keep_days: i64, now: i64) -> bool {
    expires_at(created_at, kept, keep_days).is_some_and(|at| at <= now)
}

/// What a cap asks for.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CapPlan {
    /// Snapshot ids to delete, oldest first.
    pub delete: Vec<String>,
    /// Kept snapshots alone use more than the cap, so nothing is deleted.
    pub kept_over: bool,
}

/// Oldest unkept snapshots to delete until `snaps` fit in `cap_bytes` (0 = no limit).
/// Kept snapshots count toward the total but are never deleted; if they alone pass the
/// cap, nothing is deleted. Busy snapshots are skipped.
pub fn plan_cap(snaps: &[Candidate], cap_bytes: u64, busy: impl Fn(&str) -> bool) -> CapPlan {
    if cap_bytes == 0 {
        return CapPlan::default();
    }
    let mut total: u64 = snaps.iter().map(|s| s.size_bytes).sum();
    let kept: u64 = snaps.iter().filter(|s| s.kept).map(|s| s.size_bytes).sum();
    if kept > cap_bytes {
        return CapPlan {
            delete: Vec::new(),
            kept_over: true,
        };
    }
    let mut oldest: Vec<&Candidate> = snaps
        .iter()
        .filter(|s| !s.kept && s.size_bytes > 0 && !busy(&s.id))
        .collect();
    oldest.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    let mut delete = Vec::new();
    for s in oldest {
        if total <= cap_bytes {
            break;
        }
        total -= s.size_bytes;
        delete.push(s.id.clone());
    }
    CapPlan {
        delete,
        kept_over: false,
    }
}

// ---------------------------------------------------------------------------
// The sweeper.

/// Run [`sweep`] at startup and then hourly.
pub fn spawn_sweeper(app: App) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SWEEP_EVERY);
        loop {
            tick.tick().await;
            sweep(&app).await;
        }
    });
}

/// Delete expired snapshots, then enforce the snapshot caps. Returns how many went.
pub async fn sweep(app: &App) -> usize {
    let t = now();
    let rows: Vec<(String, String, String, i64, bool, i64)> = match app
        .db
        .call(|c| {
            let mut s = c.prepare(
                "SELECT s.id, s.instance_id, s.name, s.created_at, s.kept, i.snapshot_keep_days
                 FROM snapshots s JOIN instances i ON i.id = s.instance_id",
            )?;
            let rows = s.query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })?;
            rows.collect()
        })
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "snapshot sweep");
            return 0;
        }
    };
    let mut deleted = 0;
    for (id, iid, name, created, kept, keep_days) in rows {
        if !is_expired(created, kept, keep_days, t) {
            continue;
        }
        let reason = format!("older than {keep_days} days");
        if remove(app, &iid, &id, &name, &reason).await {
            deleted += 1;
        }
    }
    deleted + enforce_caps(app, None).await
}

/// Delete one snapshot for the sweeper unless it's busy; logs it.
async fn remove(app: &App, instance_id: &str, id: &str, name: &str, reason: &str) -> bool {
    let Some(_claim) = Claim::take(id) else {
        return false;
    };
    match delete(app, instance_id, id).await {
        Ok(()) => {
            tracing::info!(instance = %instance_id, snapshot = %id, name = %name, reason, "snapshot deleted automatically");
            true
        }
        Err(e) => {
            tracing::warn!(instance = %instance_id, snapshot = %id, error = %e, "automatic snapshot delete");
            false
        }
    }
}

/// Every snapshot with its size, by Mac. Only Macs in `only` when given.
async fn measured(app: &App, only: Option<&str>) -> HashMap<String, Vec<(Candidate, String)>> {
    let only = only.map(str::to_owned);
    let rows: Vec<(String, String, String, String, i64, bool)> = app
        .db
        .call(move |c| {
            let mut s = c.prepare(
                "SELECT id, instance_id, name, tag, created_at, kept FROM snapshots
                 WHERE ?1 IS NULL OR instance_id = ?1",
            )?;
            let rows = s.query_map([only], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })?;
            rows.collect()
        })
        .await
        .unwrap_or_default();
    // Each snapshot with its tag, by Mac; sizes come from one `qemu-img info` per Mac.
    let mut by_mac: HashMap<String, Vec<(Candidate, String, String)>> = HashMap::new();
    for (id, iid, name, tag, created_at, kept) in rows {
        let candidate = Candidate {
            id,
            instance_id: iid.clone(),
            created_at,
            kept,
            size_bytes: 0,
        };
        by_mac.entry(iid).or_default().push((candidate, name, tag));
    }
    let mut out = HashMap::new();
    for (iid, snaps) in by_mac {
        let sizes = app
            .fleet
            .hv()
            .snapshot_sizes(&app.fleet.files(&iid))
            .await
            .unwrap_or_default();
        let list = snaps
            .into_iter()
            .map(|(mut c, name, tag)| {
                c.size_bytes = sizes.get(&tag).copied().unwrap_or(0);
                (c, name)
            })
            .collect();
        out.insert(iid, list);
    }
    out
}

/// Per-Mac caps in GB (0 = no limit), for `only` or every Mac.
async fn mac_caps(app: &App, only: Option<&str>) -> HashMap<String, u64> {
    let only = only.map(str::to_owned);
    app.db
        .call(move |c| {
            let mut s = c.prepare(
                "SELECT id, snapshot_max_gb FROM instances WHERE snapshot_max_gb > 0 AND (?1 IS NULL OR id = ?1)",
            )?;
            let rows = s.query_map([only], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
        .await
        .unwrap_or_default()
}

/// Delete the oldest unkept snapshots past the per-Mac caps (only `instance_id`'s
/// when given) and the server-wide cap. Returns how many went.
pub async fn enforce_caps(app: &App, instance_id: Option<&str>) -> usize {
    let caps = mac_caps(app, instance_id).await;
    let server_gb = crate::storage::snapshot_total_gb(app).await;
    if caps.is_empty() && server_gb == 0 {
        return 0;
    }
    // The server cap needs every Mac; otherwise only the capped ones.
    let scope = if server_gb > 0 { None } else { instance_id };
    let mut all = measured(app, scope).await;
    let mut deleted = 0;
    for (iid, cap_gb) in caps {
        let Some(snaps) = all.get_mut(&iid) else {
            continue;
        };
        let candidates: Vec<Candidate> = snaps.iter().map(|(c, _)| c.clone()).collect();
        let plan = plan_cap(&candidates, cap_gb.saturating_mul(GB), is_busy);
        if plan.kept_over {
            tracing::info!(instance = %iid, cap_gb, "kept snapshots use more than the Mac's snapshot limit");
        }
        for id in &plan.delete {
            let name = snaps
                .iter()
                .find(|(c, _)| &c.id == id)
                .map(|(_, n)| n.clone())
                .unwrap_or_default();
            if remove(app, &iid, id, &name, "over the Mac's snapshot limit").await {
                deleted += 1;
            }
        }
        snaps.retain(|(c, _)| !plan.delete.contains(&c.id));
    }
    if server_gb > 0 {
        let names: HashMap<String, String> = all
            .values()
            .flatten()
            .map(|(c, n)| (c.id.clone(), n.clone()))
            .collect();
        let candidates: Vec<Candidate> = all.into_values().flatten().map(|(c, _)| c).collect();
        let plan = plan_cap(&candidates, server_gb.saturating_mul(GB), is_busy);
        if plan.kept_over {
            tracing::warn!(
                cap_gb = server_gb,
                "kept snapshots use more than the server's snapshot limit"
            );
        }
        for id in plan.delete {
            let Some(c) = candidates.iter().find(|c| c.id == id) else {
                continue;
            };
            let name = names.get(&id).cloned().unwrap_or_default();
            if remove(
                app,
                &c.instance_id,
                &id,
                &name,
                "over the server's snapshot limit",
            )
            .await
            {
                deleted += 1;
            }
        }
    }
    deleted
}

/// `SnapshotUsage`: what one Mac's snapshots use against its own cap.
#[derive(Debug, Clone, Serialize)]
pub struct Usage {
    /// Everything together.
    pub used_bytes: u64,
    /// Kept snapshots.
    pub kept_bytes: u64,
    /// The Mac's cap; `None` for no limit.
    pub limit_bytes: Option<u64>,
    /// Kept snapshots alone use more than the cap, so nothing is deleted.
    pub kept_over_limit: bool,
}

/// One Mac's snapshot usage.
pub async fn usage(app: &App, instance_id: &str) -> Usage {
    let cap_gb = mac_caps(app, Some(instance_id))
        .await
        .remove(instance_id)
        .unwrap_or(0);
    let snaps: Vec<Candidate> = measured(app, Some(instance_id))
        .await
        .remove(instance_id)
        .unwrap_or_default()
        .into_iter()
        .map(|(c, _)| c)
        .collect();
    let used = snaps.iter().map(|s| s.size_bytes).sum();
    let kept = snaps.iter().filter(|s| s.kept).map(|s| s.size_bytes).sum();
    let limit = (cap_gb > 0).then(|| cap_gb.saturating_mul(GB));
    Usage {
        used_bytes: used,
        kept_bytes: kept,
        limit_bytes: limit,
        kept_over_limit: limit.is_some_and(|l| kept > l),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    fn snap(id: &str, created_at: i64, kept: bool, size_bytes: u64) -> Candidate {
        Candidate {
            id: id.into(),
            instance_id: "m".into(),
            created_at,
            kept,
            size_bytes,
        }
    }

    #[test]
    fn retention_deletes_unkept_snapshots_older_than_keep_days() {
        let now = 100 * DAY;
        // Older than 7 days goes; exactly 7 days goes; newer stays.
        assert!(is_expired(now - 8 * DAY, false, 7, now));
        assert!(is_expired(now - 7 * DAY, false, 7, now));
        assert!(!is_expired(now - 6 * DAY, false, 7, now));
        // Kept never goes.
        assert!(!is_expired(now - 99 * DAY, true, 7, now));
        // 0 keeps forever.
        assert!(!is_expired(0, false, 0, now));
        assert_eq!(expires_at(0, false, 0), None);
        assert_eq!(expires_at(0, true, 30), None);
        assert_eq!(expires_at(DAY, false, 30), Some(31 * DAY));
    }

    #[test]
    fn caps_delete_oldest_unkept_until_under() {
        let snaps = [
            snap("new", 3, false, 4 * GB),
            snap("old", 1, false, 4 * GB),
            snap("mid", 2, false, 4 * GB),
            snap("disk-only", 0, false, 0),
        ];
        // 12 GB under a 5 GB cap: the two oldest with memory go.
        let plan = plan_cap(&snaps, 5 * GB, |_| false);
        assert_eq!(plan.delete, ["old", "mid"]);
        assert!(!plan.kept_over);
        // Under the cap, or no cap: nothing.
        assert_eq!(plan_cap(&snaps, 12 * GB, |_| false), CapPlan::default());
        assert_eq!(plan_cap(&snaps, 0, |_| false), CapPlan::default());
    }

    #[test]
    fn caps_never_delete_kept_or_busy_snapshots() {
        let snaps = [
            snap("kept", 1, true, 4 * GB),
            snap("busy", 2, false, 4 * GB),
            snap("free", 3, false, 4 * GB),
        ];
        // Kept counts toward the total but is skipped; busy is skipped too.
        let plan = plan_cap(&snaps, 8 * GB, |id| id == "busy");
        assert_eq!(plan.delete, ["free"]);
        // Kept alone over the cap: delete nothing and say so.
        let plan = plan_cap(&snaps, 3 * GB, |_| false);
        assert_eq!(plan.delete, Vec::<String>::new());
        assert!(plan.kept_over);
    }

    #[test]
    fn claims_mark_snapshots_busy_until_dropped() {
        let claim = Claim::take("claim-test").unwrap();
        assert!(is_busy("claim-test"));
        assert!(Claim::take("claim-test").is_none());
        drop(claim);
        assert!(!is_busy("claim-test"));
    }
}
