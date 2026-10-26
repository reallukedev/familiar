//! MCP servers that apps on the Mac provide, bridged as tools next to the built-in ones.
//!
//! Each server is its own permission group (`mcp:<id>`) and stays off until the owner
//! turns it on in Permissions; the approval mode never switches one on. Finding servers
//! starts nothing on the Mac, and a server's tools are only listed once it's on.
use crate::{db::now, packs::Risk, state::App};
use dot_wire::{McpOp, McpServer, McpTool, Op};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

/// Permission group prefix.
pub const GROUP_PREFIX: &str = "mcp:";
/// Tool name prefix, so bridged tools never collide with built-in or pack tools.
const TOOL_PREFIX: &str = "mcp_";
/// MCP clients accept tool names up to this length.
const MAX_TOOL_NAME: usize = 64;
/// How long listing one server's tools may take: a stdio server may need to start, and
/// an `npx` one may download first.
const LIST_BUDGET: Duration = Duration::from_secs(45);

/// One refresh at a time per Mac, so a slow one can't save over a newer one.
static REFRESHING: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Mutex::default);

/// What the Mac last reported.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Cache {
    /// Servers in the order the Mac found them.
    #[serde(default)]
    pub servers: Vec<Entry>,
    /// When the Mac was last asked.
    #[serde(default)]
    pub checked_at: Option<i64>,
}

/// One server and what's known about its tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// As the Mac described it.
    pub server: McpServer,
    /// Tools from the last successful listing (empty until the server is turned on).
    #[serde(default)]
    pub tools: Vec<McpTool>,
    /// Why the last listing failed, for the owner.
    #[serde(default)]
    pub tools_error: Option<String>,
    /// When the tools were last listed.
    #[serde(default)]
    pub listed_at: Option<i64>,
}

/// A bridged tool as the assistant sees it.
#[derive(Debug, Clone)]
pub struct AppTool {
    /// Server id.
    pub server: String,
    /// Server display name.
    pub server_name: String,
    /// The server's fingerprint when its tools were listed; calls only run against that.
    pub fingerprint: String,
    /// Name the assistant calls (`mcp_<server>_<tool>`).
    pub name: String,
    /// The tool's own name on its server.
    pub original: String,
    /// Title.
    pub title: String,
    /// Description, ending with where the tool comes from.
    pub description: String,
    /// Argument schema.
    pub schema: Map<String, Value>,
    /// Read when the server marks it read-only, otherwise Write.
    pub risk: Risk,
    /// The server says it may destroy something (MCP's default for tools that change things).
    pub destructive: bool,
}

/// `mcp:<server>`.
pub fn group_id(server: &str) -> String {
    format!("{GROUP_PREFIX}{server}")
}

/// Whether `id` is a valid server group id (the Mac's id rules: `[a-z0-9_]{1,24}`).
pub fn is_group(id: &str) -> bool {
    id.strip_prefix(GROUP_PREFIX).is_some_and(valid_server_id)
}

fn valid_server_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 24
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The name an assistant calls a server's tool by: `mcp_<server>_<tool>`, with characters
/// MCP clients reject replaced, and a short hash keeping long names unique.
pub fn tool_name(server: &str, tool: &str) -> String {
    let clean: String = tool
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = format!("{TOOL_PREFIX}{server}_{clean}");
    if name.len() <= MAX_TOOL_NAME && clean == tool {
        return name;
    }
    // Changed or cut names get a hash of the original, so two tools never share one.
    let hash = Sha256::digest(format!("{server}\0{tool}").as_bytes());
    let suffix = format!("_{:02x}{:02x}{:02x}", hash[0], hash[1], hash[2]);
    let keep = MAX_TOOL_NAME - suffix.len();
    let mut cut = name;
    if cut.len() > keep {
        cut.truncate(keep);
    }
    cut + &suffix
}

/// The bridged tools of one server, in its order. Names are unique.
pub fn tools_of(entry: &Entry) -> Vec<AppTool> {
    let server = &entry.server;
    let from = match &server.app {
        Some(app) => format!("From {app}\u{2019}s MCP server on the Mac."),
        None => format!(
            "From the {} MCP server on the Mac (listed in {}\u{2019}s settings).",
            server.name, server.source
        ),
    };
    let mut seen = HashSet::new();
    entry
        .tools
        .iter()
        .filter_map(|t| {
            let name = tool_name(&server.id, &t.name);
            seen.insert(name.clone()).then(|| AppTool {
                server: server.id.clone(),
                server_name: server.name.clone(),
                fingerprint: server.fingerprint.clone(),
                original: t.name.clone(),
                title: t
                    .title
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| t.name.replace(['_', '-'], " ")),
                description: match t.description.as_deref().map(str::trim) {
                    Some(d) if !d.is_empty() => format!("{d}\n\n{from}"),
                    _ => from.clone(),
                },
                schema: match &t.input_schema {
                    Value::Object(m) if m.get("type").and_then(Value::as_str) == Some("object") => {
                        m.clone()
                    }
                    _ => json!({ "type": "object" })
                        .as_object()
                        .cloned()
                        .unwrap_or_default(),
                },
                risk: if t.read_only { Risk::Read } else { Risk::Write },
                destructive: !t.read_only && t.destructive,
                name,
            })
        })
        .collect()
}

/// What the Mac last reported for an instance.
pub async fn load(app: &App, instance_id: &str) -> anyhow::Result<Cache> {
    let iid = instance_id.to_owned();
    let raw: Option<String> = app
        .db
        .call(move |c| {
            c.query_row(
                "SELECT mcp_cache FROM instances WHERE id = ?1",
                [iid],
                |r| r.get(0),
            )
        })
        .await?;
    Ok(raw
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default())
}

async fn save(app: &App, instance_id: &str, cache: &Cache) -> anyhow::Result<()> {
    let (iid, raw) = (instance_id.to_owned(), serde_json::to_string(cache)?);
    app.db
        .call(move |c| {
            c.execute(
                "UPDATE instances SET mcp_cache = ?2 WHERE id = ?1",
                [iid, raw],
            )
        })
        .await?;
    Ok(())
}

/// Every bridged tool, in server order. If two servers' tools ever end up with the same
/// name, the first keeps it, so a name always means one tool.
pub fn all_tools(cache: &Cache) -> Vec<AppTool> {
    let mut seen = HashSet::new();
    cache
        .servers
        .iter()
        .flat_map(tools_of)
        .filter(|t| seen.insert(t.name.clone()))
        .collect()
}

/// Bridged tools of the servers the owner turned on.
pub async fn enabled_tools(
    app: &App,
    instance_id: &str,
    modes: &[(String, String)],
) -> anyhow::Result<Vec<Arc<AppTool>>> {
    let cache = load(app, instance_id).await?;
    Ok(all_tools(&cache)
        .into_iter()
        .filter(|t| super::mode_of(modes, &group_id(&t.server)) != "deny")
        .map(Arc::new)
        .collect())
}

/// Whether a server is still what the owner approved. Agents that predate fingerprints
/// report none, which matches an approval made with none.
pub fn same_server(approved: Option<&str>, current: &str) -> bool {
    approved.unwrap_or_default() == current
}

async fn approved_fingerprints(
    app: &App,
    instance_id: &str,
) -> anyhow::Result<HashMap<String, Option<String>>> {
    let iid = instance_id.to_owned();
    let rows: Vec<(String, Option<String>)> = app
        .db
        .call(move |c| {
            let mut s = c.prepare(
                "SELECT grp, fingerprint FROM permissions WHERE instance_id = ?1 AND grp LIKE 'mcp:%'",
            )?;
            let rows = s.query_map([iid], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(g, f)| g.strip_prefix(GROUP_PREFIX).map(|id| (id.to_owned(), f)))
        .collect())
}

/// Groups of servers the owner turned on whose command or environment has changed since.
/// They count as off until turned on again.
pub async fn changed(app: &App, instance_id: &str) -> anyhow::Result<HashSet<String>> {
    let iid = instance_id.to_owned();
    let approved: Vec<(String, Option<String>)> = app
        .db
        .call(move |c| {
            let mut s = c.prepare(
                "SELECT grp, fingerprint FROM permissions
                 WHERE instance_id = ?1 AND grp LIKE 'mcp:%' AND mode != 'deny'",
            )?;
            let rows = s.query_map([iid], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
        .await?;
    if approved.is_empty() {
        return Ok(HashSet::new());
    }
    let cache = load(app, instance_id).await?;
    Ok(approved
        .into_iter()
        .filter(|(group, fingerprint)| {
            cache.servers.iter().any(|e| {
                group_id(&e.server.id) == *group
                    && !same_server(fingerprint.as_deref(), &e.server.fingerprint)
            })
        })
        .map(|(group, _)| group)
        .collect())
}

/// Why the Mac couldn't be asked.
#[derive(Debug, thiserror::Error)]
pub enum RefreshError {
    /// The Mac isn't running or its agent isn't connected.
    #[error("The Mac isn\u{2019}t running. Start it to look for app servers.")]
    Offline,
    /// The agent in the Mac predates app servers.
    #[error("The agent in this Mac is too old to find app servers. Update it in Settings.")]
    Unsupported,
    /// Anything else.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Ask the Mac which servers it has and remember the result. Lists the tools of every
/// server that's on, or with `only`, of just that server (which the owner is turning on).
/// Tools of a server that fails to list are kept from the last time, with the failure
/// noted.
pub async fn refresh(
    app: &App,
    instance_id: &str,
    only: Option<&str>,
) -> Result<Cache, RefreshError> {
    let lock = REFRESHING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(instance_id.to_owned())
        .or_default()
        .clone();
    let _one_at_a_time = lock.lock().await;
    let link = app
        .fleet
        .machine(instance_id)
        .link()
        .ok_or(RefreshError::Offline)?;
    if !link.info.capabilities.iter().any(|c| c == "mcp") {
        return Err(RefreshError::Unsupported);
    }
    let found: Vec<McpServer> = link
        .call_as(Op::Mcp(McpOp::Discover), Duration::from_secs(15))
        .await
        .map_err(|e| anyhow::anyhow!("discover: {e}"))?;
    let old = load(app, instance_id).await?;
    let modes = super::modes(app, instance_id).await?;
    let approved = approved_fingerprints(app, instance_id).await?;
    let mut servers = Vec::with_capacity(found.len());
    for server in found {
        let previous = old.servers.iter().find(|e| e.server.id == server.id);
        let mut entry = Entry {
            tools: previous.map(|e| e.tools.clone()).unwrap_or_default(),
            tools_error: previous.and_then(|e| e.tools_error.clone()),
            listed_at: previous.and_then(|e| e.listed_at),
            server,
        };
        let list = match only {
            Some(id) => id == entry.server.id,
            None => super::mode_of(&modes, &group_id(&entry.server.id)) != "deny",
        };
        if list {
            // A server the owner is turning on now lists as it is; one that was on lists
            // only if it's still what they approved.
            let expected_fingerprint = match only {
                Some(_) => None,
                None => approved.get(&entry.server.id).cloned().flatten(),
            };
            let op = Op::Mcp(McpOp::ListTools {
                server: entry.server.id.clone(),
                expected_fingerprint,
            });
            match link.call_as::<Vec<McpTool>>(op, LIST_BUDGET).await {
                Ok(tools) => {
                    entry.tools = tools;
                    entry.tools_error = None;
                    entry.listed_at = Some(now());
                }
                Err(e) => entry.tools_error = Some(list_error(&e)),
            }
        }
        servers.push(entry);
    }
    let cache = Cache {
        servers,
        checked_at: Some(now()),
    };
    save(app, instance_id, &cache).await?;
    Ok(cache)
}

fn list_error(e: &crate::guest::LinkError) -> String {
    match e {
        crate::guest::LinkError::Guest(g) => match &g.hint {
            Some(h) => format!("{} {h}", g.message),
            None => g.message.clone(),
        },
        crate::guest::LinkError::Timeout => {
            "It didn\u{2019}t answer in time. Check the app is running, then look again.".into()
        }
        _ => "The Mac disconnected while listing its tools. Look again.".into(),
    }
}

/// One server's permission group as the portal shows it. `changed`: it was on, but its
/// command or environment changed since, so it's off until turned on again.
pub fn group_json(entry: &Entry, mode: &str, changed: bool) -> Value {
    let s = &entry.server;
    let description = match &s.app {
        Some(app) if s.source == "app" => format!("{app}\u{2019}s own MCP server."),
        _ => format!("Listed in {}\u{2019}s MCP settings.", s.source),
    };
    json!({
        "id": group_id(&s.id), "label": s.name, "description": description,
        "mode": mode, "default_mode": "deny",
        "tools": tools_of(entry).iter().map(|t| json!({ "name": t.name, "title": t.title, "risk": t.risk })).collect::<Vec<_>>(),
        "server": {
            "app": s.app, "bundle_id": s.bundle_id, "transport": s.transport, "detail": s.detail,
            "source": s.source, "tools_error": entry.tools_error, "changed": changed, "listed_at": entry.listed_at.map(crate::db::rfc3339),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dot_wire::McpTransport;

    fn entry(app: Option<&str>, tools: &[(&str, bool)]) -> Entry {
        Entry {
            server: McpServer {
                id: "xcode".into(),
                name: "Xcode".into(),
                app: app.map(str::to_owned),
                bundle_id: None,
                transport: McpTransport::Stdio,
                detail: "xcrun mcpbridge".into(),
                source: if app.is_some() { "app" } else { "Claude" }.into(),
                fingerprint: String::new(),
            },
            tools: tools
                .iter()
                .map(|(name, read_only)| McpTool {
                    name: (*name).into(),
                    title: None,
                    description: Some("Builds it.".into()),
                    input_schema: json!({ "type": "object", "properties": { "x": { "type": "string" } } }),
                    read_only: *read_only,
                    destructive: !read_only,
                })
                .collect(),
            tools_error: None,
            listed_at: None,
        }
    }

    #[test]
    fn tool_names_are_prefixed_valid_and_unique_even_when_cut() {
        assert_eq!(tool_name("xcode", "BuildProject"), "mcp_xcode_BuildProject");
        let dotted = tool_name("figma", "get.code");
        assert!(dotted.starts_with("mcp_figma_get_code_"), "{dotted}");
        // "get.code" and "get_code" clean to the same text; the hash keeps them apart.
        assert_ne!(dotted, tool_name("figma", "get_code"));
        let long_a = tool_name("server_with_long_name", &"a".repeat(80));
        let long_b = tool_name("server_with_long_name", &format!("{}b", "a".repeat(79)));
        for name in [&dotted, &long_a, &long_b] {
            assert!(name.len() <= MAX_TOOL_NAME, "{name}");
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            );
        }
        assert_ne!(long_a, long_b);
    }

    #[test]
    fn bridged_tools_keep_the_schema_say_where_they_come_from_and_map_risk() {
        let tools = tools_of(&entry(
            Some("Xcode"),
            &[("BuildProject", false), ("ReadLog", true)],
        ));
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].original, "BuildProject");
        assert_eq!(tools[0].risk, Risk::Write);
        assert_eq!(tools[1].risk, Risk::Read);
        assert_eq!(tools[1].title, "ReadLog");
        assert!(tools[0].description.starts_with("Builds it."));
        assert!(
            tools[0]
                .description
                .ends_with("From Xcode\u{2019}s MCP server on the Mac.")
        );
        assert!(tools[0].schema.contains_key("properties"));

        let from_config = tools_of(&entry(None, &[("x", true)]));
        assert!(
            from_config[0]
                .description
                .contains("listed in Claude\u{2019}s settings")
        );
    }

    #[test]
    fn a_missing_or_non_object_schema_becomes_an_empty_object_schema() {
        let mut e = entry(Some("Xcode"), &[("a", true)]);
        e.tools[0].input_schema = json!("nope");
        assert_eq!(
            Value::Object(tools_of(&e)[0].schema.clone()),
            json!({ "type": "object" })
        );
    }

    #[test]
    fn group_ids_follow_the_macs_id_rules() {
        assert!(is_group("mcp:xcode"));
        assert!(is_group("mcp:things_3"));
        for bad in [
            "mcp:",
            "mcp:Xcode",
            "mcp:a-b",
            "pack:xcode",
            &format!("mcp:{}", "a".repeat(25)),
        ] {
            assert!(!is_group(bad), "{bad}");
        }
    }
}
