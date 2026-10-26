//! The tools an assistant sees for one Mac, and the pipeline every call goes through:
//! pause switch, permission (allow / ask / deny), wake, concurrency, deadline, receipts
//! for anything that sends, the activity log and live portal events.
pub mod appmcp;
pub mod approvals;
pub mod catalog;
mod exec;
pub mod modes;
pub mod review;

use crate::{
    db::{now, rfc3339},
    packs::{Pack, Risk},
    state::App,
    vm::ReadyError,
};
use rmcp::model::{CallToolResult, ContentBlock as Content, Tool, ToolAnnotations};
use rusqlite::OptionalExtension;
use serde_json::{Map, Value, json};
use std::{sync::Arc, time::Duration};

/// Total time a tool call may take (clients typically give up around 60 s).
pub const CALL_BUDGET: Duration = Duration::from_secs(55);
/// How long a call waits for the owner's approval before handing back an approval id.
const APPROVAL_WAIT: Duration = Duration::from_secs(40);

/// Who is calling.
#[derive(Debug, Clone)]
pub struct Caller {
    /// Owner.
    pub user_id: String,
    /// The Mac the token is bound to.
    pub instance_id: String,
    /// Assistant name, for the activity log.
    pub client: Option<String>,
    /// The assistant connection (grant) the token belongs to; declines go back to it.
    pub grant_id: Option<String>,
    /// MCP session id, to group the Review list by session.
    pub session: Option<String>,
}

/// A resolved tool.
#[derive(Clone)]
pub enum Def {
    /// Built-in.
    Builtin(&'static catalog::BuiltinTool),
    /// From a pack.
    Pack(Arc<Pack>, usize),
    /// From an MCP server on the Mac.
    App(Arc<appmcp::AppTool>),
}

impl Def {
    /// Name.
    pub fn name(&self) -> &str {
        match self {
            Def::Builtin(t) => t.name,
            Def::Pack(p, i) => &p.tools[*i].name,
            Def::App(t) => &t.name,
        }
    }
    /// Title.
    pub fn title(&self) -> String {
        match self {
            Def::Builtin(t) => t.title.to_owned(),
            Def::Pack(p, i) => p.tools[*i].title.clone(),
            Def::App(t) => t.title.clone(),
        }
    }
    /// Risk.
    pub fn risk(&self) -> Risk {
        match self {
            Def::Builtin(t) => t.risk,
            Def::Pack(p, i) => p.tools[*i].risk,
            Def::App(t) => t.risk,
        }
    }
    /// Permission groups that must all allow it.
    pub fn groups(&self) -> Vec<String> {
        match self {
            Def::Builtin(t) => vec![t.group.to_owned()],
            Def::Pack(p, i) => {
                let mut g = vec![format!("pack:{}", p.id)];
                if p.tools[*i].risk == Risk::Send {
                    g.push("outgoing".into());
                }
                g
            }
            Def::App(t) => vec![appmcp::group_id(&t.server)],
        }
    }
    fn schema(&self) -> &Map<String, Value> {
        match self {
            Def::Builtin(t) => &t.schema,
            Def::Pack(p, i) => &p.tools[*i].schema,
            Def::App(t) => &t.schema,
        }
    }
    fn description(&self) -> String {
        match self {
            Def::Builtin(t) => t.description.to_owned(),
            Def::Pack(p, i) => p.tools[*i].description.clone(),
            Def::App(t) => t.description.clone(),
        }
    }

    /// The MCP tool definition.
    pub fn to_tool(&self) -> Tool {
        let risk = self.risk();
        let annotations = ToolAnnotations::with_title(self.title())
            .read_only(risk == Risk::Read)
            .destructive(match self {
                Def::App(t) => t.destructive,
                _ => matches!(risk, Risk::Send | Risk::System),
            })
            .idempotent(risk == Risk::Read)
            .open_world(
                risk == Risk::Send
                    || self.name().starts_with("safari_")
                    || matches!(self, Def::App(_)),
            );
        Tool::new(
            self.name().to_owned(),
            self.description(),
            Arc::new(self.schema().clone()),
        )
        .with_title(self.title())
        .with_annotations(annotations)
    }
}

/// Title for a tool name (activity rows of tools that no longer exist fall back to the name).
pub fn title_of(name: &str) -> String {
    catalog::builtin(name).map_or_else(|| name.replace('_', " "), |t| t.title.to_owned())
}

/// Packs enabled for an instance: built-in (on unless switched off) and custom ones.
pub async fn enabled_packs(app: &App, instance_id: &str) -> anyhow::Result<Vec<Arc<Pack>>> {
    let iid = instance_id.to_owned();
    let (switches, custom): (Vec<(String, bool)>, Vec<String>) = app
        .db
        .call(move |c| {
            let mut s =
                c.prepare("SELECT pack_id, enabled FROM enabled_packs WHERE instance_id = ?1")?;
            let switches = s
                .query_map([&iid], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?;
            let mut s = c.prepare("SELECT manifest FROM custom_packs WHERE instance_id = ?1")?;
            let custom = s
                .query_map([&iid], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            Ok((switches, custom))
        })
        .await?;
    let off = |id: &str| switches.iter().any(|(p, on)| p == id && !on);
    let mut packs: Vec<Arc<Pack>> = app
        .catalog
        .sorted()
        .into_iter()
        .filter(|p| !off(&p.id))
        .map(|p| Arc::new(p.clone()))
        .collect();
    for manifest in custom {
        match Pack::parse(&manifest) {
            Ok(p) if !off(&p.id) && !packs.iter().any(|b| b.id == p.id) => packs.push(Arc::new(p)),
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(instance = %instance_id, error = %e, "stored custom pack is invalid")
            }
        }
    }
    Ok(packs)
}

/// Every tool for an instance. Tools in groups set to "deny" are hidden.
pub async fn list(app: &App, instance_id: &str) -> anyhow::Result<Vec<Def>> {
    let modes = modes(app, instance_id).await?;
    let mut out: Vec<Def> = catalog::builtins().iter().map(Def::Builtin).collect();
    for pack in enabled_packs(app, instance_id).await? {
        for i in 0..pack.tools.len() {
            out.push(Def::Pack(pack.clone(), i));
        }
    }
    for tool in appmcp::enabled_tools(app, instance_id, &modes).await? {
        out.push(Def::App(tool));
    }
    out.retain(|d| d.groups().iter().all(|g| mode_of(&modes, g) != "deny"));
    Ok(out)
}

/// Resolve one tool by name.
pub async fn find(app: &App, instance_id: &str, name: &str) -> anyhow::Result<Option<Def>> {
    if let Some(t) = catalog::builtin(name) {
        return Ok(Some(Def::Builtin(t)));
    }
    for pack in enabled_packs(app, instance_id).await? {
        if let Some(i) = pack.tools.iter().position(|t| t.name == name) {
            return Ok(Some(Def::Pack(pack, i)));
        }
    }
    if name.starts_with("mcp_") {
        // Servers that are off resolve too, so the call is refused as off, not unknown.
        let cache = appmcp::load(app, instance_id).await?;
        let tool = appmcp::all_tools(&cache)
            .into_iter()
            .find(|t| t.name == name);
        return Ok(tool.map(|t| Def::App(Arc::new(t))));
    }
    Ok(None)
}

/// The display name of `target` (name or bundle id) when the owner switched that app off.
pub async fn disabled_app(
    app: &App,
    instance_id: &str,
    target: &str,
) -> anyhow::Result<Option<String>> {
    let iid = instance_id.to_owned();
    let (off, cache): (Vec<String>, Option<String>) = app
        .db
        .call(move |c| {
            let mut s = c.prepare(
                "SELECT pack_id FROM enabled_packs WHERE instance_id = ?1 AND enabled = 0",
            )?;
            let off = s
                .query_map([&iid], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            let cache = c.query_row(
                "SELECT apps_cache FROM instances WHERE id = ?1",
                [&iid],
                |r| r.get(0),
            )?;
            Ok((off, cache))
        })
        .await?;
    if off.is_empty() {
        return Ok(None);
    }
    // A switched-off pack stands for its app (e.g. the Messages pack for Messages).
    let off: Vec<String> = off
        .into_iter()
        .filter_map(|k| match k.strip_prefix("app:") {
            Some(_) => Some(k),
            None => app
                .catalog
                .packs
                .get(&k)
                .and_then(|p| p.bundle_id.clone())
                .map(|b| format!("app:{b}")),
        })
        .collect();
    let apps: Vec<dot_wire::AppInfo> = cache
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default();
    let t = target.trim().trim_end_matches(".app").to_lowercase();
    for key in off {
        let bundle = key.trim_start_matches("app:");
        let name = apps
            .iter()
            .find(|a| a.bundle_id.as_deref() == Some(bundle))
            .map_or_else(|| bundle.to_owned(), |a| a.name.clone());
        let pack_name = app
            .catalog
            .packs
            .values()
            .find(|p| p.bundle_id.as_deref() == Some(bundle))
            .map(|p| p.name.to_lowercase());
        if bundle.eq_ignore_ascii_case(&t)
            || name.to_lowercase() == t
            || pack_name.as_deref() == Some(t.as_str())
        {
            return Ok(Some(name));
        }
    }
    Ok(None)
}

/// Permission modes for an instance: the stored ones, plus what the Mac's approval mode
/// gives groups that have no stored setting yet (a newly enabled app, say).
pub async fn modes(app: &App, instance_id: &str) -> anyhow::Result<Vec<(String, String)>> {
    let iid = instance_id.to_owned();
    let (mut rows, approval_mode): (Vec<(String, String)>, Option<String>) = app
        .db
        .call(move |c| {
            let mut s = c.prepare("SELECT grp, mode FROM permissions WHERE instance_id = ?1")?;
            let rows = s
                .query_map([&iid], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?;
            let mode = c
                .query_row(
                    "SELECT approval_mode FROM instances WHERE id = ?1",
                    [&iid],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            Ok((rows, mode))
        })
        .await?;
    if let Some(mode) = approval_mode {
        let has = |rows: &[(String, String)], g: &str| rows.iter().any(|(r, _)| r == g);
        for g in catalog::GROUPS {
            if !has(&rows, g.id) {
                let m = self::modes::group_mode(&mode, self::modes::builtin_read_only(g.id), "");
                rows.push((g.id.to_owned(), m.to_owned()));
            }
        }
        for p in enabled_packs(app, instance_id).await? {
            let id = format!("pack:{}", p.id);
            if !has(&rows, &id) {
                let m = self::modes::group_mode(&mode, self::modes::pack_read_only(&p), "");
                rows.push((id, m.to_owned()));
            }
        }
    }
    // An app server that changed since the owner turned it on is off until they look again.
    let changed = appmcp::changed(app, instance_id).await?;
    for (group, mode) in &mut rows {
        if changed.contains(group) {
            "deny".clone_into(mode);
        }
    }
    Ok(rows)
}

/// Effective mode of a group.
pub fn mode_of(modes: &[(String, String)], group: &str) -> String {
    modes
        .iter()
        .find(|(g, _)| g == group)
        .map(|(_, m)| m.clone())
        .unwrap_or_else(|| {
            // An app's MCP server stays off until the owner turns it on.
            if group.starts_with(appmcp::GROUP_PREFIX) {
                return "deny".to_owned();
            }
            catalog::group(group)
                .map_or("allow", |g| g.default_mode)
                .to_owned()
        })
}

fn strictest<'a>(a: &'a str, b: &'a str) -> &'a str {
    let rank = |m: &str| match m {
        "deny" => 2,
        "ask" => 1,
        _ => 0,
    };
    if rank(a) >= rank(b) { a } else { b }
}

/// A failure the assistant reads.
pub fn failure(code: &str, message: impl Into<String>, extra: Value) -> CallToolResult {
    let mut body = json!({ "error_code": code, "message": message.into() });
    if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
        b.extend(e.clone());
    }
    CallToolResult::error(vec![Content::text(body.to_string())])
}

/// Run one tool call through the whole pipeline.
pub async fn call(
    app: &App,
    caller: &Caller,
    name: &str,
    args: Map<String, Value>,
) -> CallToolResult {
    if name == review::FEEDBACK_TOOL {
        return review::feedback_call(app, caller).await;
    }
    let started = std::time::Instant::now();
    let mut unasked_change = None;
    let outcome = run(
        app,
        caller,
        name,
        args.clone(),
        started,
        &mut unasked_change,
    )
    .await;
    let (mut result, activity_outcome, error_code) = match outcome {
        Ok(r) => (r, "ok", None),
        Err(Failure {
            result,
            outcome,
            code,
        }) => (*result, outcome, Some(code)),
    };
    let activity_id = record(
        app,
        caller,
        name,
        activity_outcome,
        started.elapsed(),
        exec::detail_for(name, &args),
        error_code,
    )
    .await;
    // A change made without asking goes on the owner's Review list.
    if activity_outcome == "ok"
        && result.is_error != Some(true)
        && let Some(summary) = unasked_change
    {
        review::record(app, caller, activity_id, name, summary).await;
    }
    review::attach_feedback(app, caller, &mut result).await;
    result
}

struct Failure {
    result: Box<CallToolResult>,
    outcome: &'static str,
    code: String,
}

fn fail(code: &str, message: impl Into<String>, extra: Value) -> Failure {
    let outcome = match code {
        "DENIED" | "APPROVAL_DENIED" | "PAUSED" => "denied",
        "APPROVAL_PENDING" => "pending",
        _ => "error",
    };
    Failure {
        result: Box::new(failure(code, message, extra)),
        outcome,
        code: code.to_owned(),
    }
}

async fn run(
    app: &App,
    caller: &Caller,
    name: &str,
    args: Map<String, Value>,
    started: std::time::Instant,
    unasked_change: &mut Option<String>,
) -> Result<CallToolResult, Failure> {
    let internal = |e: anyhow::Error| {
        tracing::error!(error = %format!("{e:#}"), "tool pipeline");
        fail(
            "INTERNAL",
            "Familiar hit an internal error. Try again.",
            json!({}),
        )
    };
    let iid = caller.instance_id.clone();
    let paused: Option<bool> = app
        .db
        .call(move |c| {
            c.query_row("SELECT paused FROM instances WHERE id = ?1", [iid], |r| {
                r.get(0)
            })
            .optional()
        })
        .await
        .map_err(|e| internal(e.into()))?;
    match paused {
        None => {
            return Err(fail(
                "MAC_UNAVAILABLE",
                "This Mac was deleted. Ask the person to connect another one.",
                json!({}),
            ));
        }
        Some(true) => {
            return Err(fail(
                "PAUSED",
                "The person paused their assistant's access to this Mac. Tell them; don't retry until they resume it.",
                json!({}),
            ));
        }
        Some(false) => {}
    }
    let def = find(app, &caller.instance_id, name)
        .await
        .map_err(internal)?
        .ok_or_else(|| {
            fail(
                "UNKNOWN_TOOL",
                format!("There's no tool named {name}. Refresh the tool list."),
                json!({}),
            )
        })?;
    // Permission first: a tool that's off is refused as off, whatever its arguments.
    let modes = modes(app, &caller.instance_id).await.map_err(internal)?;
    let mode = def.groups().iter().fold("allow".to_owned(), |acc, g| {
        strictest(&acc, &mode_of(&modes, g)).to_owned()
    });
    if mode == "deny"
        && let Def::App(t) = &def
    {
        return Err(fail(
            "DENIED",
            format!(
                "The person hasn't turned on the {} MCP server for their assistant. Ask them to turn it on in Familiar's Permissions for this Mac.",
                t.server_name
            ),
            json!({}),
        ));
    }
    if mode == "deny" {
        return Err(fail(
            "DENIED",
            format!(
                "The person turned off \u{201c}{}\u{201d} for this Mac. Don't try to work around it.",
                def.title()
            ),
            json!({}),
        ));
    }
    if let Err(msg) = crate::packs::validate_args(def.schema(), &args) {
        return Err(fail("INVALID_ARGS", msg, json!({})));
    }

    // Apps the owner switched off for the assistant can't be targeted by name. An app
    // server's own `app` argument means something else, and the server has its own switch.
    if !matches!(def, Def::App(_))
        && let Some(target) = args.get("app").and_then(Value::as_str)
        && let Some(name) = disabled_app(app, &caller.instance_id, target)
            .await
            .map_err(internal)?
    {
        return Err(fail(
            "DENIED",
            format!(
                "The person turned off {name} for their assistant. Don't use it unless they turn it back on."
            ),
            json!({}),
        ));
    }

    if mode == "ask" {
        let hash = approvals::args_hash(name, &args);
        let summary = exec::summary_for(&def, &args);
        match approvals::gate(
            app,
            &caller.instance_id,
            name,
            hash,
            &args,
            summary,
            caller.client.clone(),
            APPROVAL_WAIT,
        )
        .await
        .map_err(internal)?
        {
            approvals::Gate::Approved => {}
            approvals::Gate::Denied => {
                return Err(fail(
                    "APPROVAL_DENIED",
                    "The person declined this action. Don't repeat it unless they ask again.",
                    json!({}),
                ));
            }
            approvals::Gate::Pending(id) => {
                return Err(fail(
                    "APPROVAL_PENDING",
                    "This needs the person's approval in Familiar and they haven't answered yet. Tell them, then repeat the identical call after they approve; it won't ask twice.",
                    json!({ "approval_id": id, "retry_after_ms": 30_000 }),
                ));
            }
        }
    }

    // Receipts: a repeated request_id never sends twice.
    let receipt = if def.risk() == Risk::Send {
        let request_id = args
            .get("request_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        match receipts::claim(app, &caller.instance_id, name, &request_id, &args)
            .await
            .map_err(internal)?
        {
            receipts::Claim::New => Some(request_id),
            receipts::Claim::Done(result) => {
                return Ok(CallToolResult::structured(
                    json!({ "repeated": true, "result": result }),
                ));
            }
            receipts::Claim::Pending => {
                return Err(fail(
                    "OUTCOME_UNKNOWN",
                    "A call with this request_id already started and its outcome is unknown. Check whether it happened (for example in Sent mail) before sending again with a new request_id.",
                    json!({}),
                ));
            }
            receipts::Claim::Mismatch => {
                return Err(fail(
                    "INVALID_ARGS",
                    "This request_id was already used with different details. Use a new request_id for a new message.",
                    json!({}),
                ));
            }
        }
    } else {
        None
    };

    // If the client cancels before the call reaches the Mac, nothing was sent: free the id.
    let mut release_guard = receipt.as_ref().map(|rid| receipts::ReleaseOnDrop {
        app: app.clone(),
        instance_id: caller.instance_id.clone(),
        tool: name.to_owned(),
        request_id: rid.clone(),
        armed: true,
    });

    // Make the Mac ready (wakes or boots it).
    let machine = app.fleet.machine(&caller.instance_id);
    let budget = CALL_BUDGET
        .saturating_sub(started.elapsed())
        .saturating_sub(Duration::from_secs(8));
    let link = match machine
        .ensure_ready(budget.max(Duration::from_secs(5)))
        .await
    {
        Ok(link) => link,
        Err(ReadyError::Starting) => {
            return Err(fail(
                "MAC_STARTING",
                "The Mac is starting up. Wait about 30 seconds and make the same call again.",
                json!({ "retry_after_ms": 30_000, "state_detail": machine.status().detail }),
            ));
        }
        Err(ReadyError::Failed(msg)) => {
            return Err(fail("MAC_UNAVAILABLE", msg, json!({})));
        }
        Err(ReadyError::Locked) => {
            // The disk key exists only in memory after its owner signs in.
            return Err(fail(
                "MAC_LOCKED",
                format!(
                    "This Mac is encrypted and locked because the Familiar server restarted. Ask the person to sign in at {} to unlock it, then try again.",
                    app.config.origin()
                ),
                json!({ "sign_in_url": format!("{}/signin", app.config.origin()) }),
            ));
        }
    };
    let Ok(_permit) = machine.calls.try_acquire() else {
        return Err(fail(
            "BUSY",
            "Too many calls are running on this Mac. Wait for them to finish, then try once more.",
            json!({ "retry_after_ms": 2_000 }),
        ));
    };
    machine.touch();
    touch_instance(app, &caller.instance_id).await;
    let remaining = CALL_BUDGET
        .saturating_sub(started.elapsed())
        .max(Duration::from_secs(3));
    if let Some(g) = release_guard.as_mut() {
        g.armed = false;
    }
    let ctx = exec::Ctx {
        app,
        caller,
        machine: &machine,
        link,
        deadline: remaining,
    };
    if mode == "allow" && def.risk() != Risk::Read {
        *unasked_change = Some(exec::summary_for(&def, &args));
    }
    let result = exec::run(&ctx, &def, &args).await;
    if let Some(rid) = &receipt {
        receipts::finish(app, &caller.instance_id, name, rid, &result).await;
    }
    result.map_err(|e| fail(&e.code, e.message, e.extra))
}

async fn touch_instance(app: &App, instance_id: &str) {
    let iid = instance_id.to_owned();
    let _ = app
        .db
        .call(move |c| {
            c.execute(
                "UPDATE instances SET last_active_at = ?2 WHERE id = ?1",
                (iid, now()),
            )
        })
        .await;
}

async fn record(
    app: &App,
    caller: &Caller,
    tool: &str,
    outcome: &'static str,
    duration: Duration,
    detail: Option<String>,
    error_code: Option<String>,
) -> Option<i64> {
    let (iid, tool_s, client) = (
        caller.instance_id.clone(),
        tool.to_owned(),
        caller.client.clone(),
    );
    let ms = i64::try_from(duration.as_millis()).unwrap_or(i64::MAX);
    let t = now();
    let res = app
        .db
        .call(move |c| {
            c.execute(
                "INSERT INTO activity (instance_id, at, tool, outcome, duration_ms, detail, error_code, client)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![iid, t, tool_s, outcome, ms, detail, error_code, client],
            )?;
            Ok((c.last_insert_rowid(), detail, error_code, client))
        })
        .await;
    match res {
        Ok((id, detail, error_code, client)) => {
            app.events.instance(
                &caller.instance_id,
                "activity",
                json!({
                    "id": id.to_string(), "at": rfc3339(t), "tool": tool, "title": title_of(tool),
                    "outcome": outcome, "duration_ms": ms, "detail": detail,
                    "error_code": error_code, "client": client
                }),
            );
            Some(id)
        }
        Err(e) => {
            tracing::warn!(error = %e, "activity not recorded");
            None
        }
    }
}

/// Error from executing a tool, shaped for the assistant.
#[derive(Debug)]
pub struct ToolError {
    /// Stable code.
    pub code: String,
    /// Sentence.
    pub message: String,
    /// Extra fields (hint, retry_after_ms).
    pub extra: Value,
}

mod receipts {
    //! Durable once-only records for tools that send.
    use super::ToolError;
    use crate::{db::now, state::App};
    use rmcp::model::CallToolResult;
    use rusqlite::OptionalExtension;
    use serde_json::{Map, Value};

    /// Releases a pending receipt if dropped while still armed.
    pub struct ReleaseOnDrop {
        pub app: App,
        pub instance_id: String,
        pub tool: String,
        pub request_id: String,
        pub armed: bool,
    }

    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            if self.armed {
                let (app, i, t, r) = (
                    self.app.clone(),
                    self.instance_id.clone(),
                    self.tool.clone(),
                    self.request_id.clone(),
                );
                tokio::spawn(async move { release(&app, &i, &t, &r).await });
            }
        }
    }

    pub enum Claim {
        New,
        Done(Value),
        Pending,
        Mismatch,
    }

    pub async fn claim(
        app: &App,
        instance_id: &str,
        tool: &str,
        request_id: &str,
        args: &Map<String, Value>,
    ) -> anyhow::Result<Claim> {
        let hash = super::approvals::args_hash(tool, args);
        let (iid, tool, rid) = (
            instance_id.to_owned(),
            tool.to_owned(),
            request_id.to_owned(),
        );
        Ok(app
            .db
            .call(move |c| {
                let tx = c.transaction()?;
                let existing: Option<(Vec<u8>, String, Option<String>)> = tx
                    .query_row(
                        "SELECT args_hash, status, result FROM receipts WHERE instance_id = ?1 AND tool = ?2 AND request_id = ?3",
                        [&iid, &tool, &rid],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()?;
                let claim = match existing {
                    Some((h, _, _)) if h != hash => Claim::Mismatch,
                    Some((_, s, r)) if s == "done" => {
                        Claim::Done(r.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or(Value::Null))
                    }
                    Some((_, s, _)) if s == "pending" => Claim::Pending,
                    _ => {
                        tx.execute(
                            "INSERT INTO receipts (instance_id, tool, request_id, args_hash, status, created_at)
                             VALUES (?1, ?2, ?3, ?4, 'pending', ?5)
                             ON CONFLICT DO UPDATE SET status = 'pending', args_hash = excluded.args_hash",
                            rusqlite::params![iid, tool, rid, hash, now()],
                        )?;
                        Claim::New
                    }
                };
                tx.commit()?;
                Ok(claim)
            })
            .await?)
    }

    /// The call never reached the Mac; allow the same request_id again.
    pub async fn release(app: &App, instance_id: &str, tool: &str, request_id: &str) {
        let (iid, tool, rid) = (
            instance_id.to_owned(),
            tool.to_owned(),
            request_id.to_owned(),
        );
        let _ = app
            .db
            .call(move |c| {
                c.execute(
                    "DELETE FROM receipts WHERE instance_id = ?1 AND tool = ?2 AND request_id = ?3 AND status = 'pending'",
                    [iid, tool, rid],
                )
            })
            .await;
    }

    pub async fn finish(
        app: &App,
        instance_id: &str,
        tool: &str,
        request_id: &str,
        result: &Result<CallToolResult, ToolError>,
    ) {
        // Definite failures before anything was sent can be retried with the same id;
        // timeouts and disconnects stay pending because the outcome is unknown.
        let (status, body) = match result {
            Ok(r) => ("done", r.structured_content.clone().map(|v| v.to_string())),
            Err(e)
                if matches!(
                    e.code.as_str(),
                    "INVALID_ARGS" | "NOT_FOUND" | "PERMISSION_DENIED"
                ) =>
            {
                ("failed", None)
            }
            Err(_) => return,
        };
        let (iid, tool, rid) = (
            instance_id.to_owned(),
            tool.to_owned(),
            request_id.to_owned(),
        );
        let _ = app
            .db
            .call(move |c| {
                c.execute(
                    "UPDATE receipts SET status = ?4, result = ?5 WHERE instance_id = ?1 AND tool = ?2 AND request_id = ?3",
                    rusqlite::params![iid, tool, rid, status, body],
                )
            })
            .await;
    }
}
