//! Storage: what each person's Macs keep on the server, and the limits on it
//! (docs/home/API.md "Storage").
use super::{InstanceView, instance_view, owned_instance, publish_instance};
use crate::{
    auth::Session,
    error::{ApiError, ApiResult},
    state::App,
    storage,
    vm::State as MacState,
};
use axum::{
    Json,
    extract::{Path, State},
};
use serde_json::{Map, Value};

/// Allowed values for one limit, and the sentence shown when a value is outside them.
/// Matches `RANGES` in home-web/src/lib/storage.ts.
struct Range {
    field: &'static str,
    min: u64,
    max: u64,
    message: &'static str,
}

const QUOTA: Range = Range {
    field: "quota_gb",
    min: 0,
    max: 100_000,
    message: "Choose 0 (no limit) to 100,000 GB.",
};
const MAX_DISK: Range = Range {
    field: "max_disk_gb",
    min: 64,
    max: 4096,
    message: "Choose 64 to 4,096 GB.",
};
const DEFAULT_DISK: Range = Range {
    field: "default_disk_gb",
    min: 64,
    max: 4096,
    message: "Choose 64 to 4,096 GB.",
};
const MAX_SNAPSHOTS: Range = Range {
    field: "max_snapshots",
    min: 0,
    max: 100,
    message: "Choose 0 (no limit) to 100 snapshots.",
};
const RESERVE: Range = Range {
    field: "reserve_gb",
    min: 5,
    max: 10_000,
    message: "Choose 5 to 10,000 GB.",
};
const SNAPSHOT_TOTAL: Range = Range {
    field: "snapshot_total_gb",
    min: 0,
    max: 100_000,
    message: "Choose 0 (no limit) to 100,000 GB.",
};

impl Range {
    fn check(&self, v: &Value) -> ApiResult<u64> {
        v.as_u64()
            .filter(|n| (self.min..=self.max).contains(n))
            .ok_or_else(|| ApiError::invalid(self.field, self.message))
    }
    /// The value when the key is present (`null` is refused: a default can't be cleared).
    fn read(&self, body: &Map<String, Value>) -> ApiResult<Option<u64>> {
        body.get(self.field).map(|v| self.check(v)).transpose()
    }
    /// `None`: leave unchanged; `Some(None)`: clear; `Some(Some(n))`: set.
    fn read_override(&self, body: &Map<String, Value>) -> ApiResult<Option<Option<u64>>> {
        match body.get(self.field) {
            None => Ok(None),
            Some(Value::Null) => Ok(Some(None)),
            Some(v) => self.check(v).map(|n| Some(Some(n))),
        }
    }
}

/// `GET /api/storage`.
pub async fn mine(State(app): State<App>, s: Session) -> ApiResult<Json<Value>> {
    Ok(Json(storage::mine(&app, &s.user.id).await?))
}

/// `POST /api/instances/:id/storage/discard-memory`: a hibernated Mac becomes stopped
/// and its saved memory is deleted.
pub async fn discard_memory(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
) -> ApiResult<Json<InstanceView>> {
    let id = owned_instance(&app, &s, &id).await?;
    let m = app.fleet.machine(&id);
    if m.status().state != MacState::Hibernated {
        return Err(ApiError::conflict(
            "Only a hibernated Mac has saved memory to discard.",
        ));
    }
    // With no QEMU running, stopping only deletes the saved memory and marks it stopped.
    m.stop(false).await.map_err(ApiError::unavailable)?;
    app.storage.forget(&id);
    tracing::info!(instance = %id, user = %s.user.username, "saved memory discarded");
    publish_instance(&app, &id).await;
    Ok(Json(instance_view(&app, &id).await?))
}

/// `GET /api/admin/storage`.
pub async fn admin(State(app): State<App>, s: Session) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    Ok(Json(storage::admin(&app).await?))
}

/// `PUT /api/admin/storage/defaults`: any of `StorageDefaults`.
pub async fn set_defaults(
    State(app): State<App>,
    s: Session,
    Json(body): Json<Map<String, Value>>,
) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    s.require_confirmed()?;
    let quota = QUOTA.read(&body)?;
    let max_disk = MAX_DISK.read(&body)?;
    let max_snapshots = MAX_SNAPSHOTS.read(&body)?;
    let default_disk = DEFAULT_DISK.read(&body)?;
    let reserve = RESERVE.read(&body)?;
    let snapshot_total = SNAPSHOT_TOTAL.read(&body)?;
    if default_disk.is_some() || max_disk.is_some() {
        let current = storage::defaults(&app).await?;
        let largest = max_disk.unwrap_or(current.limits.max_disk_gb);
        if default_disk.unwrap_or(current.default_disk_gb) > largest {
            return Err(ApiError::invalid(
                if default_disk.is_some() {
                    "default_disk_gb"
                } else {
                    "max_disk_gb"
                },
                format!(
                    "The default disk can't be larger than the largest disk allowed ({largest} GB)."
                ),
            ));
        }
    }
    // The supervisor reads `reserve_gb` from settings on every check, so this applies at once.
    storage::set_defaults(
        &app,
        quota,
        max_disk,
        max_snapshots,
        default_disk,
        reserve,
        snapshot_total,
    )
    .await?;
    tracing::info!(admin = %s.user.username, "storage defaults changed");
    Ok(Json(storage::admin(&app).await?))
}

/// `PUT /api/admin/users/:id/storage`: any of `StorageOverrides`; `null` clears one.
pub async fn set_overrides(
    State(app): State<App>,
    s: Session,
    Path(id): Path<String>,
    Json(body): Json<Map<String, Value>>,
) -> ApiResult<Json<Value>> {
    s.require_admin()?;
    s.require_confirmed()?;
    // Columns from `storage::OVERRIDE_COLUMNS`.
    let changes: Vec<(&'static str, Option<i64>)> = [
        ("storage_quota_gb", QUOTA.read_override(&body)?),
        ("max_disk_gb", MAX_DISK.read_override(&body)?),
        ("max_snapshots", MAX_SNAPSHOTS.read_override(&body)?),
    ]
    .into_iter()
    .filter_map(|(column, change)| {
        change.map(|v| (column, v.map(|n| i64::try_from(n).unwrap_or(i64::MAX))))
    })
    .collect();
    let uid = id.clone();
    let found = app
        .db
        .call(move |c| {
            let tx = c.transaction()?;
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM users WHERE id = ?1)",
                [&uid],
                |r| r.get(0),
            )?;
            if !exists {
                return Ok(false);
            }
            for (column, value) in changes {
                tx.execute(
                    &format!("UPDATE users SET {column} = ?2 WHERE id = ?1"),
                    rusqlite::params![uid, value],
                )?;
            }
            tx.commit()?;
            Ok(true)
        })
        .await?;
    if !found {
        return Err(ApiError::not_found("person"));
    }
    tracing::info!(user = %id, admin = %s.user.username, "storage limits changed");
    Ok(Json(storage::admin(&app).await?))
}
