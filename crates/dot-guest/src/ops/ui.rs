//! Accessibility tree, search and element actions.
//!
//! Refs are unique across the snapshots the agent keeps (the last [`KEEP_SNAPSHOTS`]),
//! numbered in document order within each snapshot. A ref from an older snapshot, or
//! one whose element changed role, is [`dot_wire::GuestErrorCode::StaleRef`].
use super::{Ctx, input};
use crate::error::{self, OpResult};
use crate::script::{self, sources};
use dot_wire::{
    GuestErrorCode, PointerArgs, Rect, UiAction, UiActionArgs, UiFindArgs, UiNode, UiTree,
    UiTreeArgs,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::time::Duration;

/// Snapshots kept for resolving refs.
pub const KEEP_SNAPSHOTS: usize = 3;
const DEFAULT_DEPTH: u32 = 12;
const MAX_DEPTH: u32 = 40;
const DEFAULT_NODES: u32 = 400;
const MAX_NODES: u32 = 3000;
const DEFAULT_FIND_LIMIT: u32 = 25;
const MAX_FIND_LIMIT: u32 = 500;
/// Time kept back for the script to return partial results before the deadline.
const SCRIPT_MARGIN: Duration = Duration::from_millis(1500);

/// Refs from recent snapshots.
#[derive(Debug, Default)]
pub struct UiStore {
    next_snapshot: u64,
    next_ref: u64,
    snapshots: VecDeque<Snapshot>,
}

#[derive(Debug)]
struct Snapshot {
    refs: HashMap<String, StoredRef>,
}

#[derive(Debug, Clone)]
struct StoredRef {
    pid: i64,
    path: Value,
    role: String,
}

impl UiStore {
    /// Record a snapshot; returns its id and the refs, in node order.
    fn record(&mut self, pid: i64, nodes: &[RawNode]) -> (u64, Vec<String>) {
        self.next_snapshot += 1;
        let mut refs = HashMap::with_capacity(nodes.len());
        let mut names = Vec::with_capacity(nodes.len());
        for node in nodes {
            self.next_ref += 1;
            let name = format!("e{}", self.next_ref);
            refs.insert(
                name.clone(),
                StoredRef {
                    pid,
                    path: node.path.clone(),
                    role: node.role.clone(),
                },
            );
            names.push(name);
        }
        self.snapshots.push_back(Snapshot { refs });
        while self.snapshots.len() > KEEP_SNAPSHOTS {
            self.snapshots.pop_front();
        }
        (self.next_snapshot, names)
    }

    fn lookup(&self, reference: &str) -> Option<StoredRef> {
        self.snapshots
            .iter()
            .rev()
            .find_map(|s| s.refs.get(reference))
            .cloned()
    }
}

#[derive(Debug, Deserialize)]
struct RawTree {
    app: String,
    pid: i64,
    truncated: bool,
    nodes: Vec<RawNode>,
}

#[derive(Debug, Deserialize)]
struct RawNode {
    path: Value,
    depth: u32,
    role: String,
    subrole: Option<String>,
    title: Option<String>,
    value: Option<String>,
    description: Option<String>,
    frame: Option<Rect>,
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default)]
    focused: bool,
    #[serde(default)]
    actions: Vec<String>,
}

fn yes() -> bool {
    true
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.is_empty())
}

/// Accessibility failures from the scripts carry no hint; add where to fix it.
fn with_accessibility_hint(mut e: dot_wire::GuestError) -> dot_wire::GuestError {
    if e.code == GuestErrorCode::PermissionDenied && e.hint.is_none() {
        e.hint = Some(error::HINT_ACCESSIBILITY.to_owned());
    }
    e
}

fn finish(ctx: &Ctx, raw: RawTree) -> UiTree {
    let (snapshot, refs) = ctx
        .state
        .ui
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .record(raw.pid, &raw.nodes);
    let nodes = raw
        .nodes
        .into_iter()
        .zip(refs)
        .map(|(n, reference)| UiNode {
            reference,
            depth: n.depth,
            role: n.role,
            subrole: non_empty(n.subrole),
            title: non_empty(n.title),
            value: n.value,
            description: non_empty(n.description),
            frame: n.frame,
            enabled: n.enabled,
            focused: n.focused,
            actions: n.actions,
        })
        .collect();
    UiTree {
        snapshot,
        app: raw.app,
        nodes,
        truncated: raw.truncated,
    }
}

/// [`dot_wire::Op::UiTree`].
pub async fn tree(ctx: &Ctx, args: UiTreeArgs) -> OpResult<UiTree> {
    let max_depth = args.max_depth.unwrap_or(DEFAULT_DEPTH).clamp(1, MAX_DEPTH);
    let max_nodes = args.max_nodes.unwrap_or(DEFAULT_NODES).clamp(1, MAX_NODES);
    let raw: RawTree = script::jxa(
        ctx.deadline,
        sources::UI,
        &json!({
            "mode": "tree",
            "app": args.app,
            "max_depth": max_depth,
            "max_nodes": max_nodes,
            "include_menus": args.include_menus,
            "budget_ms": ctx.budget_ms(SCRIPT_MARGIN),
        }),
    )
    .await
    .map_err(with_accessibility_hint)?;
    Ok(finish(ctx, raw))
}

/// [`dot_wire::Op::UiFind`].
pub async fn find(ctx: &Ctx, args: UiFindArgs) -> OpResult<UiTree> {
    let text = args.text.filter(|t| !t.trim().is_empty());
    let role = args.role.filter(|r| !r.trim().is_empty());
    if text.is_none() && role.is_none() {
        return Err(error::invalid("Give `text`, `role` or both to search for."));
    }
    let limit = args
        .limit
        .unwrap_or(DEFAULT_FIND_LIMIT)
        .clamp(1, MAX_FIND_LIMIT);
    let raw: RawTree = script::jxa(
        ctx.deadline,
        sources::UI,
        &json!({
            "mode": "find",
            "app": args.app,
            "text": text,
            "role": role,
            "limit": limit,
            "budget_ms": ctx.budget_ms(SCRIPT_MARGIN),
        }),
    )
    .await
    .map_err(with_accessibility_hint)?;
    Ok(finish(ctx, raw))
}

#[derive(Debug, Deserialize)]
struct ActionResult {
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    click: Option<(f64, f64)>,
}

/// [`dot_wire::Op::UiAction`].
pub async fn action(ctx: &Ctx, args: UiActionArgs) -> OpResult {
    let stored = ctx
        .state
        .ui
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .lookup(&args.reference);
    let Some(stored) = stored else {
        return Err(dot_wire::GuestError::new(
            GuestErrorCode::StaleRef,
            format!(
                "`{}` is not from one of the last {KEEP_SNAPSHOTS} UI snapshots. Read the UI again.",
                args.reference
            ),
        ));
    };
    if let UiAction::Perform { name } = &args.action
        && name.trim().is_empty()
    {
        return Err(error::invalid(
            "Give the AX action name to perform, e.g. AXIncrement.",
        ));
    }
    let _input = ctx.state.input_lock.lock().await;
    let result: ActionResult = script::jxa(
        ctx.deadline,
        sources::UI,
        &json!({
            "mode": "action",
            "pid": stored.pid,
            "path": stored.path,
            "role": stored.role,
            "action": args.action,
        }),
    )
    .await
    .map_err(with_accessibility_hint)?;
    if let Some((x, y)) = result.click {
        input::post_pointer(
            ctx,
            &PointerArgs::Click {
                x,
                y,
                button: dot_wire::Button::Left,
                count: 1,
                modifiers: Vec::new(),
            },
        )
        .await?;
        return Ok(json!({ "clicked": [x, y] }));
    }
    Ok(match result.value {
        Some(value) => json!({ "value": value }),
        None => json!({}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(role: &str, path: Value) -> RawNode {
        RawNode {
            path,
            depth: 1,
            role: role.into(),
            subrole: None,
            title: None,
            value: None,
            description: None,
            frame: None,
            enabled: true,
            focused: false,
            actions: vec![],
        }
    }

    #[test]
    fn refs_stay_valid_for_the_last_snapshots_and_then_go_stale() {
        let mut store = UiStore::default();
        let (first, refs) = store.record(
            10,
            &[
                node("AXWindow", json!(["w", 0])),
                node("AXButton", json!(["w", 0, 1])),
            ],
        );
        assert_eq!(refs, ["e1", "e2"]);
        let (second, refs2) = store.record(10, &[node("AXWindow", json!(["w", 0]))]);
        assert_eq!(second, first + 1);
        assert_eq!(refs2, ["e3"], "refs never repeat across snapshots");
        assert_eq!(
            store.lookup("e2").map(|r| r.role),
            Some("AXButton".to_owned())
        );
        store.record(10, &[]);
        store.record(10, &[]);
        assert!(
            store.lookup("e2").is_none(),
            "snapshot 1 fell out of the window"
        );
        assert!(store.lookup("e3").is_some());
    }
}
