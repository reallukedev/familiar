//! [`Op::Mcp`](dot_wire::Op::Mcp): MCP servers that apps on the Mac provide.
//!
//! The host only ever names a server by the id [`McpOp::Discover`] gave it. Every
//! `ListTools` and `CallTool` runs discovery again and looks the id up, so what gets
//! started or connected to always comes from the Mac's own files, never from the host.
use super::{Ctx, to_value};
use crate::error::{self, OpResult};
use dot_wire::{GuestError, GuestErrorCode, McpOp, McpTool};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::time::Instant;

pub mod discover;
mod http;
mod redact;
mod rpc;
mod stdio;

use discover::{Found, Launch};

/// Sessions unused this long are closed.
const IDLE: Duration = Duration::from_secs(10 * 60);
/// How often idle sessions are looked for.
const REAP_EVERY: Duration = Duration::from_secs(60);
/// Timeouts in a row after which a server is stopped instead of kept.
const MAX_TIMEOUTS: u8 = 2;
/// Most tools listed for one server.
const MAX_TOOLS: usize = 200;
/// Most `tools/list` pages read for one server.
const MAX_PAGES: usize = 50;
/// Kept back from the call's deadline so a timeout is reported, not cut off.
const REPORT_MARGIN: Duration = Duration::from_millis(150);

/// Run one MCP operation.
pub async fn run(ctx: &Ctx, op: McpOp) -> OpResult {
    let home = ctx.state.home.clone();
    let deadline = ctx
        .deadline
        .checked_sub(REPORT_MARGIN)
        .unwrap_or(ctx.deadline);
    match op {
        McpOp::Discover => {
            let found = find_all(home).await?;
            to_value(found.into_iter().map(|f| f.server).collect::<Vec<_>>())
        }
        McpOp::ListTools {
            server,
            expected_fingerprint,
        } => {
            let found = resolve(
                find_all(home).await?,
                &server,
                expected_fingerprint.as_deref(),
            )?;
            to_value(list_tools(&ctx.state.mcp, &found, &ctx.state.home, deadline).await?)
        }
        McpOp::CallTool {
            server,
            tool,
            arguments,
            expected_fingerprint,
        } => {
            if tool.trim().is_empty() {
                return Err(error::invalid("The tool name is empty."));
            }
            let found = resolve(
                find_all(home).await?,
                &server,
                expected_fingerprint.as_deref(),
            )?;
            let params = json!({"name": tool, "arguments": Value::Object(arguments)});
            ctx.state
                .mcp
                .request(&found, &ctx.state.home, deadline, "tools/call", params)
                .await
        }
    }
}

/// Discovery reads files, some of them large, so it runs off the async threads.
async fn find_all(home: PathBuf) -> OpResult<Vec<Found>> {
    tokio::task::spawn_blocking(move || discover::discover(&home))
        .await
        .map_err(|e| error::failed(format!("Could not look for MCP servers: {e}")))
}

fn resolve(found: Vec<Found>, id: &str, expected: Option<&str>) -> OpResult<Found> {
    if id.trim().is_empty() {
        return Err(error::invalid("The server id is empty."));
    }
    let found = found
        .into_iter()
        .find(|f| f.server.id == id)
        .ok_or_else(|| {
            error::not_found(format!(
                "There's no MCP server called `{id}` on this Mac any more. It may have been removed, or its app uninstalled. List the servers again to see what's there now."
            ))
        })?;
    // What runs must be what the owner approved, even if a file changed since the host
    // last looked.
    if expected.is_some_and(|fp| fp != found.server.fingerprint) {
        return Err(error::not_found(format!(
            "The MCP server `{id}` changed since the person turned it on (its command or settings are different), so it won't run until they turn it on again in Familiar."
        )));
    }
    Ok(found)
}

async fn list_tools(
    pool: &Pool,
    found: &Found,
    home: &Path,
    deadline: Instant,
) -> OpResult<Vec<McpTool>> {
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let params = match &cursor {
            Some(cursor) => json!({"cursor": cursor}),
            None => json!({}),
        };
        let page = pool
            .request(found, home, deadline, "tools/list", params)
            .await?;
        let listed = page.get("tools").and_then(Value::as_array);
        tools.extend(listed.into_iter().flatten().filter_map(to_tool));
        if tools.len() >= MAX_TOOLS {
            tools.truncate(MAX_TOOLS);
            break;
        }
        match page.get("nextCursor").and_then(Value::as_str) {
            Some(next) if !next.is_empty() && cursor.as_deref() != Some(next) => {
                cursor = Some(next.to_owned());
            }
            _ => break,
        }
    }
    Ok(tools)
}

fn to_tool(tool: &Value) -> Option<McpTool> {
    let name = tool.get("name")?.as_str()?.to_owned();
    let annotations = tool.get("annotations");
    let hint = |key: &str| {
        annotations
            .and_then(|a| a.get(key))
            .and_then(Value::as_bool)
    };
    let text = |value: Option<&Value>| value.and_then(Value::as_str).map(ToOwned::to_owned);
    let read_only = hint("readOnlyHint").unwrap_or(false);
    Some(McpTool {
        name,
        title: text(tool.get("title")).or_else(|| text(annotations.and_then(|a| a.get("title")))),
        description: text(tool.get("description")),
        input_schema: tool
            .get("inputSchema")
            .filter(|schema| schema.is_object())
            .cloned()
            .unwrap_or_else(|| json!({"type": "object"})),
        read_only,
        // MCP: destructiveHint defaults to true, and only means something when the tool
        // isn't read-only.
        destructive: !read_only && hint("destructiveHint").unwrap_or(true),
    })
}

/// Live MCP sessions, one per server id.
pub struct Pool {
    inner: Arc<Inner>,
}

struct Inner {
    slots: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<Slot>>>>,
    search: stdio::SearchPath,
    reaper: AtomicBool,
}

#[derive(Default)]
struct Slot {
    live: Option<Live>,
}

struct Live {
    launch: Launch,
    session: Session,
    last_used: Instant,
    /// Timeouts in a row. A server that misses [`MAX_TIMEOUTS`] is stopped.
    timeouts: u8,
}

enum Session {
    Stdio(stdio::Session),
    Http(http::Session),
}

/// Holds a server's slot during one call. If the call is dropped while a request is
/// in flight (the host cancelled it), a stdio server's state is unknown: it is told to
/// cancel the request, then stopped.
struct Active {
    slot: tokio::sync::OwnedMutexGuard<Slot>,
    in_flight: bool,
}

impl Drop for Active {
    fn drop(&mut self) {
        if !self.in_flight {
            return;
        }
        let settled = match &self.slot.live {
            Some(Live {
                session: Session::Http(session),
                ..
            }) => session.settled(),
            _ => false,
        };
        if settled {
            return;
        }
        if let Some(Live {
            session: Session::Stdio(session),
            ..
        }) = self.slot.live.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(session.abandon());
        }
    }
}

impl Default for Pool {
    fn default() -> Self {
        Self::with_search(stdio::SearchPath::LoginShell)
    }
}

impl std::fmt::Debug for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let servers = self.inner.slots.lock().map(|s| s.len()).unwrap_or(0);
        f.debug_struct("Pool").field("servers", &servers).finish()
    }
}

impl Pool {
    fn with_search(search: stdio::SearchPath) -> Self {
        Self {
            inner: Arc::new(Inner {
                slots: std::sync::Mutex::default(),
                search,
                reaper: AtomicBool::new(false),
            }),
        }
    }

    /// Send one request to `found`, starting or restarting its session as needed.
    async fn request(
        &self,
        found: &Found,
        home: &Path,
        deadline: Instant,
        method: &str,
        params: Value,
    ) -> OpResult<Value> {
        let name = &found.server.name;
        reap(&self.inner, Instant::now());
        let slot = self.slot(&found.server.id);
        let Ok(slot) = tokio::time::timeout_at(deadline, slot.lock_owned()).await else {
            return Err(GuestError::new(
                GuestErrorCode::Busy,
                format!(
                    "{name} is still busy with an earlier call. Try again when it has finished."
                ),
            ));
        };
        let mut active = Active {
            slot,
            in_flight: false,
        };
        // The config changed since this session started: start the new command.
        if active
            .slot
            .live
            .as_ref()
            .is_some_and(|live| live.launch != found.launch)
        {
            active.slot.live = None;
        }
        if active.slot.live.is_none() {
            let session = self.start(found, home, deadline).await?;
            self.start_reaper();
            active.slot.live = Some(Live {
                launch: found.launch.clone(),
                session,
                last_used: Instant::now(),
                timeouts: 0,
            });
        }
        active.in_flight = true;
        let Some(live) = active.slot.live.as_mut() else {
            return Err(error::failed(format!("{name} could not be started.")));
        };
        let outcome = match &mut live.session {
            Session::Stdio(session) => session.request(method, params, deadline).await,
            Session::Http(session) => session.request(method, params, deadline).await,
        };
        live.last_used = Instant::now();
        live.timeouts = match &outcome {
            Err(rpc::Fail::Timeout(_)) => live.timeouts.saturating_add(1),
            _ => 0,
        };
        let stop = match &outcome {
            Err(rpc::Fail::Broken(_)) => true,
            Err(rpc::Fail::Timeout(_)) => live.timeouts >= MAX_TIMEOUTS,
            _ => false,
        };
        active.in_flight = false;
        if stop {
            // Dropping a stdio session kills the server's process group.
            active.slot.live = None;
        }
        match outcome {
            Ok(result) => Ok(result),
            Err(rpc::Fail::Rpc(message)) => Err(error::failed(format!(
                "{name} returned an error: {message}"
            ))),
            Err(rpc::Fail::Timeout(_)) if stop => Err(error::timeout(format!(
                "{name} did not answer {MAX_TIMEOUTS} times in a row, so it was stopped. The next call starts it again."
            ))),
            Err(rpc::Fail::Timeout(e) | rpc::Fail::Broken(e)) => Err(e),
        }
    }

    async fn start(&self, found: &Found, home: &Path, deadline: Instant) -> OpResult<Session> {
        let server = &found.server;
        match &found.launch {
            Launch::Stdio { command, args, env } => {
                let spawn = stdio::Spawn {
                    name: &server.name,
                    command,
                    args,
                    env,
                    cwd: home,
                };
                stdio::Session::start(&self.inner.search, &spawn, deadline)
                    .await
                    .map(Session::Stdio)
            }
            Launch::Http { url } => {
                http::Session::start(&server.name, server.app.as_deref(), url, deadline)
                    .await
                    .map(Session::Http)
            }
        }
    }

    fn slot(&self, id: &str) -> Arc<tokio::sync::Mutex<Slot>> {
        let mut slots = self
            .inner
            .slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slots.entry(id.to_owned()).or_default().clone()
    }

    /// Close idle sessions every minute, for as long as the pool exists.
    fn start_reaper(&self) {
        if self.inner.reaper.swap(true, Ordering::Relaxed) {
            return;
        }
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(REAP_EVERY).await;
                let Some(inner) = weak.upgrade() else { break };
                reap(&inner, Instant::now());
            }
        });
    }
}

/// Drop sessions idle for longer than [`IDLE`]. Busy sessions are left alone.
fn reap(inner: &Inner, now: Instant) {
    let slots: Vec<_> = inner
        .slots
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .values()
        .cloned()
        .collect();
    for slot in slots {
        if let Ok(mut slot) = slot.try_lock()
            && slot
                .live
                .as_ref()
                .is_some_and(|live| now.saturating_duration_since(live.last_used) > IDLE)
        {
            slot.live = None;
        }
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "Test assertions fail immediately on malformed setup"
    )]
    use super::*;
    use dot_wire::{McpServer, McpTransport};
    use std::collections::BTreeMap;

    /// A fake MCP server in POSIX sh. It logs each start and each `initialized`
    /// notification to `$LOG`, and answers by method:
    /// - `tools/list`: first pings us and sends a notification, then page 1 with a
    ///   cursor; with the cursor, page 2. It refuses to go on if our ping answer is wrong.
    /// - `tools/call` `boom`: a JSON-RPC error. `slow`: answers after a second. `hang`:
    ///   never answers. Others: a result.
    ///
    /// It also logs `notifications/cancelled`, and writes the environment variables
    /// that must never reach it to `$LOG.env`.
    const FAKE: &str = r#"
echo start >> "$LOG"
printf '%s|%s|%s\n' "${NODE_OPTIONS:-none}" "${DYLD_INSERT_LIBRARIES:-none}" "$PATH" > "$LOG.env"
reply() { printf '{"jsonrpc":"2.0","id":%s,%s}\n' "$1" "$2"; }
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/^.*"id":\([0-9][0-9]*\).*$/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      reply "$id" '"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"1"}}' ;;
    *'"method":"notifications/initialized"'*)
      echo initialized >> "$LOG" ;;
    *'"method":"notifications/cancelled"'*)
      echo cancelled >> "$LOG" ;;
    *'"method":"tools/list"'*'"cursor":"p2"'*)
      reply "$id" '"result":{"tools":[{"name":"wipe","description":"Erase it all"}]}' ;;
    *'"method":"tools/list"'*)
      echo 'not json, just a log line'
      printf '%s\n' '{"jsonrpc":"2.0","id":"s1","method":"ping"}'
      IFS= read -r pong
      case "$pong" in
        *'"id":"s1"'*'"result":{}'*) ;;
        *) reply "$id" '"error":{"code":-1,"message":"bad pong"}'; continue ;;
      esac
      printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}'
      printf '%s\n' '{"jsonrpc":"2.0","id":"s2","method":"sampling/createMessage","params":{}}'
      IFS= read -r refused
      case "$refused" in *'-32601'*) ;; *) continue ;; esac
      reply "$id" '"result":{"tools":[{"name":"look","title":"Look","inputSchema":{"type":"object","properties":{"q":{"type":"string"}}},"annotations":{"readOnlyHint":true}},{"name":"poke","annotations":{"title":"Poke","destructiveHint":false}},{"title":"no name"}],"nextCursor":"p2"}' ;;
    *'"method":"tools/call"'*'"name":"boom"'*)
      reply "$id" '"error":{"code":-32602,"message":"Unknown tool: boom"}' ;;
    *'"method":"tools/call"'*'"name":"slow"'*)
      sleep 1; reply "$id" '"result":{"late":true}' ;;
    *'"method":"tools/call"'*'"name":"hang"'*)
      sleep 30 ;;
    *'"method":"tools/call"'*)
      reply "$id" '"result":{"content":[{"type":"text","text":"done"}],"isError":false,"_meta":{"x":1}}' ;;
  esac
done
"#;

    struct Fixture {
        dir: tempfile::TempDir,
        found: Found,
        pool: Pool,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake.sh");
        std::fs::write(&script, FAKE).unwrap();
        let log = dir.path().join("log");
        let found = Found {
            server: McpServer {
                id: "fake".into(),
                name: "Fake".into(),
                app: None,
                bundle_id: None,
                transport: McpTransport::Stdio,
                detail: String::new(),
                source: "test".into(),
                fingerprint: String::new(),
            },
            // A bare name, found through the pool's PATH.
            launch: Launch::Stdio {
                command: "sh".into(),
                args: vec![script.to_string_lossy().into_owned()],
                env: BTreeMap::from([
                    ("LOG".to_owned(), log.to_string_lossy().into_owned()),
                    (
                        "NODE_OPTIONS".to_owned(),
                        "--require /tmp/evil.js".to_owned(),
                    ),
                    (
                        "DYLD_INSERT_LIBRARIES".to_owned(),
                        "/tmp/evil.dylib".to_owned(),
                    ),
                ]),
            },
        };
        Fixture {
            dir,
            found,
            pool: Pool::with_search(stdio::SearchPath::Fixed(PATH.to_owned())),
        }
    }

    impl Fixture {
        fn log(&self) -> String {
            std::fs::read_to_string(self.dir.path().join("log")).unwrap_or_default()
        }
        async fn call(&self, tool: &str, wait: Duration) -> OpResult<Value> {
            let params = json!({"name": tool, "arguments": {"a": 1}});
            self.pool
                .request(
                    &self.found,
                    self.dir.path(),
                    Instant::now() + wait,
                    "tools/call",
                    params,
                )
                .await
        }
    }

    const LONG: Duration = Duration::from_secs(10);
    const SHORT: Duration = Duration::from_millis(300);
    const PATH: &str = "/usr/bin:/bin";

    #[tokio::test]
    async fn tools_are_listed_across_pages_while_the_server_pings_and_notifies() {
        let fx = fixture();
        let tools = list_tools(&fx.pool, &fx.found, fx.dir.path(), Instant::now() + LONG)
            .await
            .unwrap();
        let summary: Vec<_> = tools
            .iter()
            .map(|t| {
                (
                    t.name.as_str(),
                    t.title.as_deref(),
                    t.read_only,
                    t.destructive,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("look", Some("Look"), true, false),
                ("poke", Some("Poke"), false, false),
                ("wipe", None, false, true),
            ]
        );
        assert_eq!(tools[0].input_schema["properties"]["q"]["type"], "string");
        assert_eq!(tools[2].input_schema, json!({"type": "object"}));
        assert_eq!(tools[2].description.as_deref(), Some("Erase it all"));
        // One process, one handshake, for both pages.
        assert_eq!(fx.log(), "start\ninitialized\n");
    }

    #[tokio::test]
    async fn results_pass_through_and_server_errors_keep_the_session() {
        let fx = fixture();
        let result = fx.call("anything", LONG).await.unwrap();
        assert_eq!(
            result,
            json!({"content": [{"type": "text", "text": "done"}], "isError": false, "_meta": {"x": 1}})
        );
        let err = fx.call("boom", LONG).await.unwrap_err();
        assert_eq!(err.code, GuestErrorCode::Failed);
        assert_eq!(err.message, "Fake returned an error: Unknown tool: boom");
        fx.call("again", LONG).await.unwrap();
        assert_eq!(fx.log(), "start\ninitialized\n");
    }

    #[tokio::test]
    async fn a_slow_answer_is_cancelled_and_skipped_and_a_waiting_call_is_busy() {
        let fx = fixture();
        fx.call("warm", LONG).await.unwrap();
        let err = fx.call("slow", SHORT).await.unwrap_err();
        assert_eq!(err.code, GuestErrorCode::Timeout);
        // The late answer to "slow" has an old id and is skipped; the session is kept.
        let (after, behind) = tokio::join!(fx.call("after", LONG), fx.call("behind", SHORT));
        assert_eq!(after.unwrap()["content"][0]["text"], "done");
        assert_eq!(behind.unwrap_err().code, GuestErrorCode::Busy);
        assert_eq!(fx.log(), "start\ninitialized\ncancelled\n");
    }

    #[tokio::test]
    async fn a_server_that_misses_two_answers_in_a_row_is_stopped_and_restarted() {
        let fx = fixture();
        assert_eq!(
            fx.call("hang", SHORT).await.unwrap_err().code,
            GuestErrorCode::Timeout
        );
        let err = fx.call("hang", SHORT).await.unwrap_err();
        assert_eq!(err.code, GuestErrorCode::Timeout);
        assert!(err.message.contains("so it was stopped"), "{}", err.message);
        fx.call("after", LONG).await.unwrap();
        assert_eq!(fx.log(), "start\ninitialized\nstart\ninitialized\n");
    }

    #[tokio::test]
    async fn a_call_dropped_mid_request_stops_the_server() {
        let fx = fixture();
        let dropped = tokio::time::timeout(SHORT, fx.call("hang", LONG)).await;
        assert!(dropped.is_err());
        fx.call("after", LONG).await.unwrap();
        assert_eq!(fx.log(), "start\ninitialized\nstart\ninitialized\n");
    }

    #[tokio::test]
    async fn a_changed_config_restarts_the_server_without_code_loading_variables() {
        let mut fx = fixture();
        fx.call("first", LONG).await.unwrap();
        if let Launch::Stdio { env, .. } = &mut fx.found.launch {
            env.insert("EXTRA".into(), "1".into());
        }
        fx.call("second", LONG).await.unwrap();
        assert_eq!(fx.log(), "start\ninitialized\nstart\ninitialized\n");
        let env = std::fs::read_to_string(fx.dir.path().join("log.env")).unwrap();
        assert_eq!(env, format!("none|none|{PATH}\n"));
    }

    #[tokio::test]
    async fn a_server_that_exits_reports_redacted_stderr_and_missing_things_are_named() {
        let mut fx = fixture();
        fx.found.launch = Launch::Stdio {
            command: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "echo \"login failed for $LOGIN_PASS with sk-proj-abcdef123456\" >&2; exit 3"
                    .into(),
            ],
            env: BTreeMap::from([("LOGIN_PASS".to_owned(), "hunter2-secret".to_owned())]),
        };
        let err = fx.call("x", LONG).await.unwrap_err();
        assert_eq!(err.code, GuestErrorCode::Failed);
        assert_eq!(
            err.message,
            format!(
                "Fake stopped before it answered. It said: login failed for {h} with {h}",
                h = redact::HIDDEN
            )
        );

        fx.found.launch = Launch::Stdio {
            command: "no-such-mcp-server".into(),
            args: Vec::new(),
            env: BTreeMap::new(),
        };
        let err = fx.call("x", LONG).await.unwrap_err();
        assert_eq!(err.code, GuestErrorCode::Unsupported);
        assert!(err.message.contains(PATH), "{}", err.message);

        let err = resolve(vec![fx.found.clone()], "gone", None).unwrap_err();
        assert_eq!(err.code, GuestErrorCode::NotFound);
        assert!(err.message.contains("`gone`"), "{}", err.message);

        // A server that changed since the owner approved it is refused, not run.
        let id = fx.found.server.id.clone();
        let current = fx.found.server.fingerprint.clone();
        let err = resolve(vec![fx.found.clone()], &id, Some("approved-before")).unwrap_err();
        assert!(err.message.contains("changed since"), "{}", err.message);
        assert!(resolve(vec![fx.found.clone()], &id, Some(&current)).is_ok());
    }

    #[tokio::test]
    async fn only_sessions_idle_for_ten_minutes_are_closed() {
        let fx = fixture();
        fx.call("first", LONG).await.unwrap();
        reap(&fx.pool.inner, Instant::now() + IDLE / 2);
        assert!(fx.pool.slot("fake").lock().await.live.is_some());
        reap(
            &fx.pool.inner,
            Instant::now() + IDLE + Duration::from_secs(1),
        );
        assert!(fx.pool.slot("fake").lock().await.live.is_none());
    }
}
