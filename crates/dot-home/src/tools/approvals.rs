//! "Ask first": a tool call waits for the owner to approve it in the portal.
//!
//! An approval covers exactly one tool with exactly the same arguments, once. If the
//! owner doesn't answer in time, the assistant gets the approval id and can repeat the
//! identical call later; it then reuses the same approval instead of asking again.
use crate::{
    crypto,
    db::{now, rfc3339},
    error::{ApiError, ApiResult},
    state::App,
};
use rusqlite::OptionalExtension;
use serde::Serialize;
use std::{collections::HashMap, sync::Mutex, time::Duration};
use tokio::sync::watch;

/// How long an approval request stays answerable.
pub const EXPIRES_S: i64 = 15 * 60;

/// Live waiters by approval id.
#[derive(Default)]
pub struct Waiters(Mutex<HashMap<String, watch::Sender<String>>>);

impl Waiters {
    fn channel(&self, id: &str) -> watch::Receiver<String> {
        let mut map = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.entry(id.to_owned())
            .or_insert_with(|| watch::channel("pending".to_owned()).0)
            .subscribe()
    }
    fn resolve(&self, id: &str, status: &str) {
        let map = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(tx) = map.get(id) {
            let _ = tx.send(status.to_owned());
        }
    }
    fn drop_id(&self, id: &str) {
        let mut map = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if map.get(id).is_some_and(|tx| tx.receiver_count() == 0) {
            map.remove(id);
        }
    }
}

/// Portal view.
#[derive(Debug, Clone, Serialize)]
pub struct ApprovalView {
    /// Id.
    pub id: String,
    /// Instance.
    pub instance_id: String,
    /// Tool name.
    pub tool: String,
    /// Tool title.
    pub title: String,
    /// What will happen.
    pub summary: String,
    /// Assistant name.
    pub client: Option<String>,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub expires_at: String,
    /// `pending`, `approved`, `denied`, `expired`.
    pub status: String,
    /// Every argument of the call, so the owner sees exactly what they approve.
    pub arguments: serde_json::Value,
}

const COLUMNS: &str =
    "id, instance_id, tool, summary, client, created_at, expires_at, status, arguments";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ApprovalView> {
    let tool: String = r.get(2)?;
    let mut status: String = r.get(7)?;
    let expires: i64 = r.get(6)?;
    if status == "pending" && expires < now() {
        status = "expired".into();
    }
    if status == "used" {
        status = "approved".into();
    }
    Ok(ApprovalView {
        id: r.get(0)?,
        instance_id: r.get(1)?,
        title: crate::tools::title_of(&tool),
        tool,
        summary: r.get(3)?,
        client: r.get(4)?,
        created_at: rfc3339(r.get(5)?),
        expires_at: rfc3339(expires),
        status,
        arguments: r
            .get::<_, Option<String>>(8)?
            .and_then(|a| serde_json::from_str(&a).ok())
            .unwrap_or_else(|| serde_json::json!({})),
    })
}

/// What the gate decided.
pub enum Gate {
    /// Go ahead.
    Approved,
    /// The owner said no.
    Denied,
    /// Still waiting; the assistant should repeat the call later.
    Pending(String),
}

/// Hash binding an approval to a tool and its exact arguments.
pub fn args_hash(tool: &str, args: &serde_json::Map<String, serde_json::Value>) -> Vec<u8> {
    // serde_json maps are sorted (no preserve_order feature), so this is canonical.
    let text = serde_json::to_string(args).unwrap_or_default();
    crypto::sha256(format!("{tool}\n{text}").as_bytes())
}

/// Ask the owner, waiting up to `wait`.
#[allow(clippy::too_many_arguments)]
pub async fn gate(
    app: &App,
    instance_id: &str,
    tool: &str,
    hash: Vec<u8>,
    arguments: &serde_json::Map<String, serde_json::Value>,
    summary: String,
    client: Option<String>,
    wait: Duration,
) -> anyhow::Result<Gate> {
    let t = now();
    let iid = instance_id.to_owned();
    let tool_s = tool.to_owned();
    // Reuse an approved-but-unused or still-pending approval for the same call.
    let existing = app
        .db
        .call(move |c| {
            c.query_row(
                "SELECT id, status FROM approvals
                 WHERE instance_id = ?1 AND tool = ?2 AND args_hash = ?3 AND expires_at > ?4
                   AND status IN ('pending', 'approved') ORDER BY created_at DESC LIMIT 1",
                rusqlite::params![iid, tool_s, hash, t],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map(|o| (o, hash))
        })
        .await?;
    let (existing, hash) = existing;
    let id = match existing {
        Some((id, status)) if status == "approved" => {
            return Ok(if consume(app, &id).await? {
                Gate::Approved
            } else {
                Gate::Pending(id)
            });
        }
        Some((id, _)) => id,
        None => {
            let id = crypto::random_id::<10>();
            let (iid, tool_s, id2, summary2, client2) = (
                instance_id.to_owned(),
                tool.to_owned(),
                id.clone(),
                summary,
                client,
            );
            let args_json = serde_json::to_string(arguments)?;
            let view = app
                .db
                .call(move |c| {
                    c.execute(
                        "INSERT INTO approvals (id, instance_id, tool, summary, args_hash, client, status, created_at, expires_at, arguments)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?8, ?9)",
                        rusqlite::params![id2, iid, tool_s, summary2, hash, client2, t, t + EXPIRES_S, args_json],
                    )?;
                    c.query_row(&format!("SELECT {COLUMNS} FROM approvals WHERE id = ?1"), [&id2], row)
                })
                .await?;
            app.events
                .instance(instance_id, "approval", serde_json::to_value(&view)?);
            crate::push::approval_created(app, &view);
            id
        }
    };
    let mut rx = app.approvals.channel(&id);
    let decided = tokio::time::timeout(wait, async {
        loop {
            let s = rx.borrow_and_update().clone();
            if s != "pending" {
                return s;
            }
            if rx.changed().await.is_err() {
                return "pending".to_owned();
            }
        }
    })
    .await;
    drop(rx);
    app.approvals.drop_id(&id);
    match decided.as_deref() {
        Ok("approved") => Ok(if consume(app, &id).await? {
            Gate::Approved
        } else {
            Gate::Pending(id)
        }),
        Ok("denied") => Ok(Gate::Denied),
        _ => Ok(Gate::Pending(id)),
    }
}

/// Use an approval. Only one caller can win, so one approval runs the action once.
async fn consume(app: &App, id: &str) -> anyhow::Result<bool> {
    let id = id.to_owned();
    let n = app
        .db
        .call(move |c| {
            c.execute(
                "UPDATE approvals SET status = 'used' WHERE id = ?1 AND status = 'approved'",
                [id],
            )
        })
        .await?;
    Ok(n == 1)
}

/// The owner's decision from the portal.
pub async fn decide(
    app: &App,
    instance_id: &str,
    approval_id: &str,
    approve: bool,
) -> ApiResult<ApprovalView> {
    let (iid, aid) = (instance_id.to_owned(), approval_id.to_owned());
    let status = if approve { "approved" } else { "denied" };
    let t = now();
    let view = app
        .db
        .call(move |c| {
            let n = c.execute(
                "UPDATE approvals SET status = ?3 WHERE id = ?1 AND instance_id = ?2 AND status = 'pending' AND expires_at > ?4",
                rusqlite::params![aid, iid, status, t],
            )?;
            let v = c
                .query_row(
                    &format!("SELECT {COLUMNS} FROM approvals WHERE id = ?1 AND instance_id = ?2"),
                    [&aid, &iid],
                    row,
                )
                .optional()?;
            Ok((n, v))
        })
        .await?;
    match view {
        (_, None) => Err(ApiError::not_found("request")),
        (0, Some(v)) => Err(ApiError::conflict(format!(
            "This request was already {}.",
            if v.status == "expired" {
                "expired".to_owned()
            } else {
                format!("answered ({})", v.status)
            }
        ))),
        (_, Some(v)) => {
            app.approvals.resolve(approval_id, status);
            app.events.instance(
                instance_id,
                "approval",
                serde_json::to_value(&v).unwrap_or_default(),
            );
            Ok(v)
        }
    }
}

/// Pending first, then the last 50.
pub async fn list(app: &App, instance_id: &str) -> ApiResult<Vec<ApprovalView>> {
    let iid = instance_id.to_owned();
    Ok(app
        .db
        .call(move |c| {
            let mut s = c.prepare(&format!(
                "SELECT {COLUMNS} FROM approvals WHERE instance_id = ?1
                 ORDER BY (status = 'pending' AND expires_at > ?2) DESC, created_at DESC LIMIT 50"
            ))?;
            let rows = s.query_map(rusqlite::params![iid, now()], row)?;
            rows.collect()
        })
        .await?)
}
