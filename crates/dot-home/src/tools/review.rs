//! The Review list: changes the assistant made without asking first, for the owner to
//! look over at the end of a session. Accepting just files an item away. Declining (with
//! an optional note) tells the assistant connection that made the change: the notice is
//! put in front of the next tool result that connection gets, or returned by
//! `get_owner_feedback`, exactly once.
use super::Caller;
use crate::{
    crypto,
    db::{now, rfc3339},
    error::{ApiError, ApiResult},
    state::App,
};
use rmcp::model::{CallToolResult, ContentBlock as Content, Tool, ToolAnnotations};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::sync::Arc;

/// The MCP tool that returns undelivered declines.
pub const FEEDBACK_TOOL: &str = "get_owner_feedback";

/// Longest note the owner can send with a decline.
pub const NOTE_MAX: usize = 1000;

/// One reviewable change.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewItem {
    /// Id.
    pub id: String,
    /// Instance.
    pub instance_id: String,
    /// The activity row of the call.
    pub activity_id: Option<String>,
    /// Tool name.
    pub tool: String,
    /// Tool title.
    pub title: String,
    /// What it did.
    pub summary: String,
    /// Assistant name.
    pub client: Option<String>,
    /// MCP session, for grouping.
    pub session: Option<String>,
    /// The assistant connection (grant) that made the change.
    pub connection_id: Option<String>,
    /// RFC 3339.
    pub created_at: String,
    /// `pending`, `accepted` or `declined`.
    pub status: String,
    /// The owner's note with a decline.
    pub note: Option<String>,
    /// RFC 3339.
    pub decided_at: Option<String>,
    /// RFC 3339; when a decline reached the assistant.
    pub delivered_at: Option<String>,
}

const COLUMNS: &str = "id, instance_id, activity_id, tool, summary, client, session, grant_id, created_at, status, note, decided_at, delivered_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewItem> {
    let tool: String = r.get(3)?;
    Ok(ReviewItem {
        id: r.get(0)?,
        instance_id: r.get(1)?,
        activity_id: r.get::<_, Option<i64>>(2)?.map(|a| a.to_string()),
        title: super::title_of(&tool),
        tool,
        summary: r.get(4)?,
        client: r.get(5)?,
        session: r.get(6)?,
        connection_id: r.get(7)?,
        created_at: rfc3339(r.get(8)?),
        status: r.get(9)?,
        note: r.get(10)?,
        decided_at: r.get::<_, Option<i64>>(11)?.map(rfc3339),
        delivered_at: r.get::<_, Option<i64>>(12)?.map(rfc3339),
    })
}

/// What a new item records.
pub struct NewItem<'a> {
    /// Instance.
    pub instance_id: &'a str,
    /// Activity row.
    pub activity_id: Option<i64>,
    /// Tool name.
    pub tool: &'a str,
    /// What it did.
    pub summary: &'a str,
    /// Assistant name.
    pub client: Option<&'a str>,
    /// Assistant connection.
    pub grant_id: Option<&'a str>,
    /// MCP session.
    pub session: Option<&'a str>,
    /// Unix seconds.
    pub at: i64,
}

/// Insert one pending item.
pub fn insert(c: &Connection, item: &NewItem<'_>) -> rusqlite::Result<ReviewItem> {
    let id = crypto::random_id::<10>();
    c.execute(
        "INSERT INTO review_items (id, instance_id, activity_id, tool, summary, client, grant_id, session, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            id,
            item.instance_id,
            item.activity_id,
            item.tool,
            item.summary,
            item.client,
            item.grant_id,
            item.session,
            item.at
        ],
    )?;
    c.query_row(
        &format!("SELECT {COLUMNS} FROM review_items WHERE id = ?1"),
        [&id],
        row,
    )
}

/// Declines for this connection the assistant hasn't seen yet, marked delivered in the
/// same transaction so each one is handed over once.
pub fn take_undelivered(c: &mut Connection, grant_id: &str) -> rusqlite::Result<Vec<ReviewItem>> {
    let tx = c.transaction()?;
    let items = {
        let mut s = tx.prepare(&format!(
            "SELECT {COLUMNS} FROM review_items
             WHERE grant_id = ?1 AND status = 'declined' AND delivered_at IS NULL ORDER BY decided_at, created_at, rowid"
        ))?;
        s.query_map([grant_id], row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let t = now();
    for i in &items {
        tx.execute(
            "UPDATE review_items SET delivered_at = ?2 WHERE id = ?1",
            rusqlite::params![i.id, t],
        )?;
    }
    tx.commit()?;
    Ok(items)
}

/// The sentence the assistant reads for each decline.
pub fn notice(items: &[ReviewItem]) -> String {
    items
        .iter()
        .map(|i| {
            let mut s = format!(
                "The owner declined your earlier change: {} ({}).",
                i.title, i.summary
            );
            if let Some(n) = i.note.as_deref().filter(|n| !n.is_empty()) {
                s.push_str(&format!(" Their note: {n}"));
            }
            s
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Record a change the assistant made without asking.
pub async fn record(
    app: &App,
    caller: &Caller,
    activity_id: Option<i64>,
    tool: &str,
    summary: String,
) {
    let (iid, tool_s, client, grant, session) = (
        caller.instance_id.clone(),
        tool.to_owned(),
        caller.client.clone(),
        caller.grant_id.clone(),
        caller.session.clone(),
    );
    let res = app
        .db
        .call(move |c| {
            insert(
                c,
                &NewItem {
                    instance_id: &iid,
                    activity_id,
                    tool: &tool_s,
                    summary: &summary,
                    client: client.as_deref(),
                    grant_id: grant.as_deref(),
                    session: session.as_deref(),
                    at: now(),
                },
            )
        })
        .await;
    match res {
        Ok(item) => changed(app, &caller.instance_id, &item),
        Err(e) => tracing::warn!(error = %e, "review item not recorded"),
    }
}

fn changed(app: &App, instance_id: &str, item: &ReviewItem) {
    app.events.instance(
        instance_id,
        "review",
        serde_json::to_value(item).unwrap_or_default(),
    );
    // The pending count lives in the instance view.
    app.events.instance(instance_id, "state", json!({}));
}

/// Put undelivered declines in front of a tool result for this connection.
pub async fn attach_feedback(app: &App, caller: &Caller, result: &mut CallToolResult) {
    let Some(grant) = caller.grant_id.clone() else {
        return;
    };
    let items = match app.db.call(move |c| take_undelivered(c, &grant)).await {
        Ok(items) => items,
        Err(e) => {
            tracing::warn!(error = %e, "owner feedback not delivered");
            return;
        }
    };
    if items.is_empty() {
        return;
    }
    let text = notice(&items);
    if let Some(Value::Object(m)) = result.structured_content.as_mut() {
        m.insert("owner_feedback".into(), Value::String(text.clone()));
    }
    result.content.insert(0, Content::text(text));
    for i in &items {
        changed(app, &caller.instance_id, i);
    }
}

/// The `get_owner_feedback` tool definition.
pub fn feedback_tool() -> Tool {
    let mut schema = Map::new();
    schema.insert("type".into(), json!("object"));
    schema.insert("properties".into(), json!({}));
    schema.insert("additionalProperties".into(), json!(false));
    Tool::new(
        FEEDBACK_TOOL,
        "Changes the person looked over in Familiar and declined since you last heard, with their notes. \
         Declines also arrive in front of your next tool result, once; call this when you want to check.",
        Arc::new(schema),
    )
    .with_title("Owner feedback")
    .with_annotations(
        ToolAnnotations::with_title("Owner feedback")
            .read_only(true)
            .destructive(false)
            .idempotent(false)
            .open_world(false),
    )
}

/// Run `get_owner_feedback`.
pub async fn feedback_call(app: &App, caller: &Caller) -> CallToolResult {
    let Some(grant) = caller.grant_id.clone() else {
        return CallToolResult::structured(json!({ "declined": [] }));
    };
    match app.db.call(move |c| take_undelivered(c, &grant)).await {
        Ok(items) => {
            for i in &items {
                changed(app, &caller.instance_id, i);
            }
            let declined: Vec<Value> = items
                .iter()
                .map(|i| {
                    json!({ "tool": i.tool, "title": i.title, "summary": i.summary, "note": i.note,
                            "made_at": i.created_at, "declined_at": i.decided_at })
                })
                .collect();
            let mut r = CallToolResult::structured(json!({ "declined": declined }));
            if !items.is_empty() {
                r.content.insert(0, Content::text(notice(&items)));
            }
            r
        }
        Err(e) => {
            tracing::error!(error = %e, "owner feedback");
            super::failure(
                "INTERNAL",
                "Familiar hit an internal error. Try again.",
                json!({}),
            )
        }
    }
}

/// `pending` items (oldest session first is up to the client) or decided ones (`done`).
pub async fn list(app: &App, instance_id: &str, status: &str) -> ApiResult<Vec<ReviewItem>> {
    let iid = instance_id.to_owned();
    let pending = match status {
        "pending" => true,
        "done" => false,
        _ => return Err(ApiError::invalid("status", "Use pending or done.")),
    };
    Ok(app
        .db
        .call(move |c| {
            let sql = if pending {
                format!(
                    "SELECT {COLUMNS} FROM review_items WHERE instance_id = ?1 AND status = 'pending'
                     ORDER BY created_at DESC, rowid DESC LIMIT 500"
                )
            } else {
                format!(
                    "SELECT {COLUMNS} FROM review_items WHERE instance_id = ?1 AND status != 'pending'
                     ORDER BY decided_at DESC, created_at DESC, rowid DESC LIMIT 200"
                )
            };
            let mut s = c.prepare(&sql)?;
            let rows = s.query_map([iid], row)?;
            rows.collect()
        })
        .await?)
}

/// Accept or decline one pending item.
pub async fn decide(
    app: &App,
    instance_id: &str,
    item_id: &str,
    accept: bool,
    note: Option<String>,
) -> ApiResult<ReviewItem> {
    let note = note
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty() && !accept);
    if note.as_ref().is_some_and(|n| n.chars().count() > NOTE_MAX) {
        return Err(ApiError::invalid(
            "note",
            format!("Keep the note under {NOTE_MAX} characters."),
        ));
    }
    let (iid, id) = (instance_id.to_owned(), item_id.to_owned());
    let status = if accept { "accepted" } else { "declined" };
    let (n, item) = app
        .db
        .call(move |c| {
            let n = c.execute(
                "UPDATE review_items SET status = ?3, note = ?4, decided_at = ?5
                 WHERE id = ?1 AND instance_id = ?2 AND status = 'pending'",
                rusqlite::params![id, iid, status, note, now()],
            )?;
            let item = c
                .query_row(
                    &format!(
                        "SELECT {COLUMNS} FROM review_items WHERE id = ?1 AND instance_id = ?2"
                    ),
                    [&id, &iid],
                    row,
                )
                .optional()?;
            Ok((n, item))
        })
        .await?;
    match (n, item) {
        (_, None) => Err(ApiError::not_found("change")),
        (0, Some(i)) => Err(ApiError::conflict(format!(
            "This change was already {}.",
            i.status
        ))),
        (_, Some(i)) => {
            changed(app, instance_id, &i);
            Ok(i)
        }
    }
}

/// Accept every pending item, just one session's, or just the given ids.
pub async fn accept_all(
    app: &App,
    instance_id: &str,
    session: Option<String>,
    ids: Option<Vec<String>>,
) -> ApiResult<usize> {
    if ids.as_ref().is_some_and(|i| i.len() > 500) {
        return Err(ApiError::invalid("ids", "Accept at most 500 at a time."));
    }
    let iid = instance_id.to_owned();
    let n = app
        .db
        .call(move |c| {
            let tx = c.transaction()?;
            let t = now();
            let n = match ids {
                Some(ids) => {
                    let mut n = 0;
                    for id in ids {
                        n += tx.execute(
                            "UPDATE review_items SET status = 'accepted', decided_at = ?3
                             WHERE instance_id = ?1 AND id = ?2 AND status = 'pending'",
                            rusqlite::params![iid, id, t],
                        )?;
                    }
                    n
                }
                None => tx.execute(
                    "UPDATE review_items SET status = 'accepted', decided_at = ?3
                     WHERE instance_id = ?1 AND status = 'pending' AND (?2 IS NULL OR session = ?2)",
                    rusqlite::params![iid, session, t],
                )?,
            };
            tx.commit()?;
            Ok(n)
        })
        .await?;
    if n > 0 {
        app.events
            .instance(instance_id, "review", json!({ "accepted": n }));
        app.events.instance(instance_id, "state", json!({}));
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> crate::db::Db {
        crate::db::Db::memory().unwrap()
    }

    async fn seed(db: &crate::db::Db) {
        db.call(|c| {
            c.execute(
                "INSERT INTO users (id, username, display_name, password_hash, role, created_at) VALUES ('u1','alex','Alex','x','member',0)",
                [],
            )?;
            c.execute(
                "INSERT INTO instances (id, user_id, name, driver, cpus, memory_mb, disk_gb, secret, created_at)
                 VALUES ('m1', 'u1', 'Studio', 'remote', 2, 4096, 64, x'00', 0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_change_becomes_a_pending_item_and_a_decline_is_delivered_once() {
        let db = setup();
        seed(&db).await;
        let item = db
            .call(|c| {
                insert(
                    c,
                    &NewItem {
                        instance_id: "m1",
                        activity_id: Some(7),
                        tool: "mac_shell",
                        summary: "Run: rm notes.txt",
                        client: Some("Claude"),
                        grant_id: Some("g1"),
                        session: Some("s1"),
                        at: 100,
                    },
                )
            })
            .await
            .unwrap();
        assert_eq!(item.status, "pending");
        assert_eq!(item.activity_id.as_deref(), Some("7"));
        assert_eq!(item.connection_id.as_deref(), Some("g1"));

        // Nothing to deliver while it's pending.
        let none = db.call(|c| take_undelivered(c, "g1")).await.unwrap();
        assert!(none.is_empty());

        let id = item.id.clone();
        db.call(move |c| {
            c.execute(
                "UPDATE review_items SET status = 'declined', note = 'Keep that file', decided_at = 200 WHERE id = ?1",
                [id],
            )
        })
        .await
        .unwrap();
        // Another connection doesn't get it.
        assert!(
            db.call(|c| take_undelivered(c, "g2"))
                .await
                .unwrap()
                .is_empty()
        );
        let first = db.call(|c| take_undelivered(c, "g1")).await.unwrap();
        assert_eq!(first.len(), 1);
        let text = notice(&first);
        assert!(text.contains("declined your earlier change"), "{text}");
        assert!(text.contains("Their note: Keep that file"), "{text}");
        let second = db.call(|c| take_undelivered(c, "g1")).await.unwrap();
        assert!(second.is_empty(), "delivered exactly once");
    }
}
