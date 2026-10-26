//! Finding MCP servers on the Mac. Only reads files and checks paths; nothing is started.
use super::http;
use super::redact::{redact_args, redact_url};
use dot_wire::{McpServer, McpTransport};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// Longest server id.
const MAX_ID: usize = 24;
/// Longest [`McpServer::detail`], in characters.
const MAX_DETAIL: usize = 200;
/// Bytes of SHA-256 in a config server's id (8 hex characters).
const HASH_BYTES: usize = 4;
/// Config files bigger than this are skipped (`~/.claude.json` grows with history).
const MAX_CONFIG: u64 = 32 * 1024 * 1024;

/// A server and how to reach it. Only [`Found::server`] ever leaves the Mac.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// What the host sees.
    pub server: McpServer,
    /// What the guest runs or connects to.
    pub launch: Launch,
}

/// How to reach a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// Start `command args...` with `env` added, and speak JSON-RPC over its stdio.
    Stdio {
        /// Program name or path.
        command: String,
        /// Arguments.
        args: Vec<String>,
        /// Extra environment. Never shown to the host.
        env: BTreeMap<String, String>,
    },
    /// Streamable HTTP at a loopback URL.
    Http {
        /// `http://127.0.0.1:.../...`
        url: String,
    },
}

/// A server that ships inside an app.
struct KnownApp {
    id: &'static str,
    name: &'static str,
    bundle: &'static str,
    bundle_id: &'static str,
    launch: KnownLaunch,
}

enum KnownLaunch {
    Stdio(&'static str, &'static [&'static str]),
    Http(&'static str),
}

/// Apps with their own MCP server. Add a row to support another one.
const KNOWN_APPS: &[KnownApp] = &[
    // Xcode 26.3 and later include a bridge to the running Xcode.
    KnownApp {
        id: "xcode",
        name: "Xcode",
        bundle: "Xcode.app",
        bundle_id: "com.apple.dt.Xcode",
        launch: KnownLaunch::Stdio("xcrun", &["mcpbridge"]),
    },
    // The Figma desktop app serves this once the person turns it on in Figma.
    KnownApp {
        id: "figma",
        name: "Figma",
        bundle: "Figma.app",
        bundle_id: "com.figma.Desktop",
        launch: KnownLaunch::Http("http://127.0.0.1:3845/mcp"),
    },
];

/// Another assistant's config file that lists MCP servers.
struct ConfigFile {
    source: &'static str,
    path: &'static str,
    key: &'static str,
}

/// In the order they are read. Earlier entries win when two list the same server.
const CONFIG_FILES: &[ConfigFile] = &[
    ConfigFile {
        source: "Claude",
        path: "Library/Application Support/Claude/claude_desktop_config.json",
        key: "mcpServers",
    },
    ConfigFile {
        source: "Claude Code",
        path: ".claude.json",
        key: "mcpServers",
    },
    ConfigFile {
        source: "Cursor",
        path: ".cursor/mcp.json",
        key: "mcpServers",
    },
    ConfigFile {
        source: "VS Code",
        path: "Library/Application Support/Code/User/mcp.json",
        key: "servers",
    },
];

/// One entry of any of those files. Each tool uses a subset of these fields.
#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "type")]
    kind: Option<String>,
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    url: Option<String>,
}

/// Servers on this Mac for the user whose home is `home`.
pub fn discover(home: &Path) -> Vec<Found> {
    let app_dirs = [PathBuf::from("/Applications"), home.join("Applications")];
    discover_in(home, &app_dirs)
}

/// Servers from apps in `app_dirs` and from config files under `home`, deduplicated,
/// with ids that only ever name one server.
pub fn discover_in(home: &Path, app_dirs: &[PathBuf]) -> Vec<Found> {
    let mut list = List::default();
    for app in KNOWN_APPS {
        if !app_dirs.iter().any(|dir| dir.join(app.bundle).is_dir()) {
            continue;
        }
        let launch = match app.launch {
            KnownLaunch::Stdio(command, args) => Launch::Stdio {
                command: command.to_owned(),
                args: args.iter().map(|a| (*a).to_owned()).collect(),
                env: BTreeMap::new(),
            },
            KnownLaunch::Http(url) => Launch::Http {
                url: url.to_owned(),
            },
        };
        list.push(
            app.id.to_owned(),
            app.name,
            Some((app.name, app.bundle_id)),
            "app",
            launch,
        );
    }
    for file in CONFIG_FILES {
        for (name, launch) in read_config(&home.join(file.path), file.key) {
            let id = config_id(&name, file.source, &launch);
            list.push(id, &name, None, file.source, launch);
        }
    }
    list.finish()
}

#[derive(Default)]
struct List {
    found: Vec<Found>,
    seen: HashSet<String>,
}

impl List {
    fn push(
        &mut self,
        id: String,
        name: &str,
        app: Option<(&str, &str)>,
        source: &str,
        launch: Launch,
    ) {
        if !self.seen.insert(same_server_key(&launch)) {
            return;
        }
        let (transport, mut detail) = match &launch {
            Launch::Stdio { command, args, .. } => {
                let detail = std::iter::once(command.clone())
                    .chain(redact_args(args))
                    .collect::<Vec<_>>()
                    .join(" ");
                (McpTransport::Stdio, detail)
            }
            Launch::Http { url } => (McpTransport::Http, redact_url(url)),
        };
        crate::util::truncate_chars(&mut detail, MAX_DETAIL);
        let fingerprint = fingerprint(source, &launch);
        self.found.push(Found {
            server: McpServer {
                id,
                fingerprint,
                name: name.to_owned(),
                app: app.map(|(name, _)| name.to_owned()),
                bundle_id: app.map(|(_, bundle)| bundle.to_owned()),
                transport,
                detail,
                source: source.to_owned(),
            },
            launch,
        });
    }

    /// Drop every server whose id another one also has. That needs two different
    /// servers whose hashes match, which is all but impossible; if it ever happens,
    /// neither is offered rather than one id standing for two servers.
    fn finish(self) -> Vec<Found> {
        let mut count = std::collections::HashMap::<&str, usize>::new();
        for found in &self.found {
            *count.entry(found.server.id.as_str()).or_default() += 1;
        }
        let clashing: HashSet<String> = count
            .into_iter()
            .filter(|(_, n)| *n > 1)
            .map(|(id, _)| id.to_owned())
            .collect();
        for id in &clashing {
            tracing::warn!(%id, "two MCP servers have the same id; neither is offered");
        }
        self.found
            .into_iter()
            .filter(|f| !clashing.contains(&f.server.id))
            .collect()
    }
}

/// The id of a server from a config file: its name's slug, then a hash of where it was
/// found and what it runs, like `things_3_1a2b3c4d`.
///
/// The host keeps the owner's Allow or Ask for each id, so an id must never come to
/// mean a different command. The hash is always added, even when the name is unique:
/// otherwise removing one of two "things" servers would hand the plain `things` id to
/// the other. Renaming a server or changing its command gives it a new id, which turns
/// it off until the owner allows it again. Only the environment can change under the
/// same id, and that changes [`fingerprint`], which the host also checks.
fn config_id(name: &str, source: &str, launch: &Launch) -> String {
    let mut hash = Sha256::new();
    hash.update(source.as_bytes());
    match launch {
        Launch::Stdio { command, args, .. } => {
            hash.update(b"\0stdio\0");
            hash.update(command.as_bytes());
            for arg in args {
                hash.update(b"\0");
                hash.update(arg.as_bytes());
            }
        }
        Launch::Http { url } => {
            hash.update(b"\0http\0");
            hash.update(url.as_bytes());
        }
    }
    let digest = crate::util::hex(&hash.finalize()[..HASH_BYTES]);
    let mut stem = slug(name);
    stem.truncate(MAX_ID - 1 - digest.len());
    format!("{}_{digest}", stem.trim_end_matches('_'))
}

/// Everything that decides what runs: the source, then the command, every argument and
/// every environment name and value, or the URL. The host keeps this with the owner's
/// approval and treats the server as off once it changes. Each field is length-prefixed
/// so no two different configs hash the same input.
fn fingerprint(source: &str, launch: &Launch) -> String {
    fn field(hash: &mut Sha256, bytes: &[u8]) {
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    }
    let mut hash = Sha256::new();
    field(&mut hash, b"dot-mcp-server-v1");
    field(&mut hash, source.as_bytes());
    match launch {
        Launch::Stdio { command, args, env } => {
            field(&mut hash, b"stdio");
            field(&mut hash, command.as_bytes());
            hash.update((args.len() as u64).to_be_bytes());
            for arg in args {
                field(&mut hash, arg.as_bytes());
            }
            // A BTreeMap iterates sorted by name.
            hash.update((env.len() as u64).to_be_bytes());
            for (name, value) in env {
                field(&mut hash, name.as_bytes());
                field(&mut hash, value.as_bytes());
            }
        }
        Launch::Http { url } => {
            field(&mut hash, b"http");
            field(&mut hash, url.as_bytes());
        }
    }
    crate::util::hex(&hash.finalize())
}

/// Two entries with the same key are the same server.
fn same_server_key(launch: &Launch) -> String {
    match launch {
        Launch::Stdio { command, args, .. } => {
            let mut key = format!("stdio\0{command}");
            for arg in args {
                key.push('\0');
                key.push_str(arg);
            }
            key
        }
        Launch::Http { url } => match http::Endpoint::parse_loopback(url) {
            Some(endpoint) => format!("http\0{}", endpoint.same_server_key()),
            None => format!("http\0{url}"),
        },
    }
}

/// The last good parse of each config file, with the size and time it had then.
struct Cached {
    stamp: (Option<SystemTime>, u64),
    servers: Vec<(String, Launch)>,
}

static CACHE: Mutex<BTreeMap<PathBuf, Cached>> = Mutex::new(BTreeMap::new());

fn cache() -> std::sync::MutexGuard<'static, BTreeMap<PathBuf, Cached>> {
    CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Servers in one config file, by name. A missing file is normal. A file is parsed
/// again only when its size or modification time changed; one that can't be parsed
/// (often a file caught half-written) keeps its last good parse.
///
/// Logs never quote the file: serde's messages include the values they choke on.
fn read_config(path: &Path, key: &str) -> Vec<(String, Launch)> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => {
            cache().remove(path);
            return Vec::new();
        }
    };
    let stamp = (meta.modified().ok(), meta.len());
    if let Some(cached) = cache().get(path)
        && cached.stamp == stamp
    {
        return cached.servers.clone();
    }
    match parse_config(path, key, meta.len()) {
        Some(servers) => {
            cache().insert(
                path.to_path_buf(),
                Cached {
                    stamp,
                    servers: servers.clone(),
                },
            );
            servers
        }
        None => cache()
            .get(path)
            .map(|cached| cached.servers.clone())
            .unwrap_or_default(),
    }
}

/// `None` when the file can't be read or isn't JSON.
fn parse_config(path: &Path, key: &str, len: u64) -> Option<Vec<(String, Launch)>> {
    if len > MAX_CONFIG {
        tracing::warn!(path = %path.display(), "MCP config file is too large; skipped");
        return None;
    }
    let text = match std::fs::read(path) {
        Ok(text) => text,
        Err(e) => {
            tracing::warn!(path = %path.display(), kind = ?e.kind(), "could not read MCP config");
            return None;
        }
    };
    let root: serde_json::Value = match serde_json::from_slice(&text) {
        Ok(root) => root,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                category = ?e.classify(),
                line = e.line(),
                column = e.column(),
                "MCP config is not valid JSON; keeping what it said before"
            );
            return None;
        }
    };
    let Some(servers) = root.get(key).and_then(serde_json::Value::as_object) else {
        return Some(Vec::new());
    };
    let servers = servers
        .iter()
        .filter_map(|(name, raw)| {
            let entry: Entry = match serde_json::from_value(raw.clone()) {
                Ok(entry) => entry,
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        category = ?e.classify(),
                        "an MCP config entry was not understood; skipped"
                    );
                    return None;
                }
            };
            launch_for(entry).map(|launch| (name.clone(), launch))
        })
        .collect();
    Some(servers)
}

/// Local stdio servers and loopback HTTP servers. Remote URLs aren't on this Mac, and
/// SSE-only servers use a transport the guest doesn't speak.
fn launch_for(entry: Entry) -> Option<Launch> {
    let kind = entry.kind.as_deref().map(str::to_ascii_lowercase);
    match (kind.as_deref(), entry.command, entry.url) {
        (Some("sse"), _, _) => None,
        (None | Some("stdio"), Some(command), _) if !command.trim().is_empty() => {
            Some(Launch::Stdio {
                command,
                args: entry.args,
                env: entry.env,
            })
        }
        (None | Some("http" | "streamable-http" | "streamablehttp"), _, Some(url))
            if http::Endpoint::parse_loopback(&url).is_some() =>
        {
            Some(Launch::Http { url })
        }
        _ => None,
    }
}

/// Lowercase letters, digits and single `_`s, at most [`MAX_ID`] characters, never empty.
fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len().min(MAX_ID));
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
        if out.len() >= MAX_ID {
            break;
        }
    }
    out.truncate(MAX_ID);
    let out = out.trim_end_matches('_');
    if out.is_empty() {
        "server".to_owned()
    } else {
        out.to_owned()
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "Test assertions fail immediately on malformed setup"
    )]
    use super::*;
    use serde_json::json;

    fn write(home: &Path, rel: &str, text: &str) {
        let path = home.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A home with every config file, overlapping entries, secrets in env and in one
    /// broken file; and an app folder with Xcode and Figma.
    fn fixture() -> (tempfile::TempDir, tempfile::TempDir) {
        let home = tempfile::tempdir().unwrap();
        let apps = tempfile::tempdir().unwrap();
        std::fs::create_dir(apps.path().join("Xcode.app")).unwrap();
        std::fs::create_dir(apps.path().join("Figma.app")).unwrap();
        let h = home.path();
        write(
            h,
            "Library/Application Support/Claude/claude_desktop_config.json",
            &json!({"mcpServers": {
                "Things 3": {"command": "npx", "args": ["-y", "things-mcp"], "env": {"THINGS_TOKEN": "sekrit-1"}},
                "xcode": {"command": "xcrun", "args": ["mcpbridge"]},
                "!!!": {"command": "/usr/local/bin/odd"},
                "broken": {"command": "x", "env": {"N": 5}}
            }})
            .to_string(),
        );
        write(
            h,
            ".claude.json",
            &json!({
                "numStartups": 4,
                "mcpServers": {
                    "things-3": {"type": "stdio", "command": "uvx", "args": ["things"], "env": {"API_KEY": "sekrit-2"}},
                    "dup of things": {"command": "npx", "args": ["-y", "things-mcp"]},
                    "local-http": {"type": "http", "url": "http://localhost:9000/mcp"},
                    "old-sse": {"type": "sse", "url": "http://127.0.0.1:9100/sse"},
                    "remote": {"type": "http", "url": "https://mcp.example.com/mcp"}
                },
                "projects": {"/x": {"mcpServers": {"project-only": {"command": "nope"}}}}
            })
            .to_string(),
        );
        write(h, ".cursor/mcp.json", "{ this is not json");
        write(
            h,
            "Library/Application Support/Code/User/mcp.json",
            &json!({"servers": {
                "figma again": {"type": "http", "url": "http://LOCALHOST:3845/mcp/"},
                "lan": {"type": "http", "url": "http://192.168.1.20:8000/mcp"},
                "lookalike": {"url": "http://localhost.evil.example:80/mcp"},
                "userinfo": {"url": "http://127.0.0.1@evil.example/mcp"},
                "v6": {"type": "http", "url": "http://[::1]:7000/mcp?x=1"},
                "A very long server name that goes on": {"command": "long-one"},
                "A very long server name that goes on and on": {"command": "long-two"}
            }})
            .to_string(),
        );
        (home, apps)
    }

    /// Each id without its hash, and the source. Checks that config ids carry one.
    fn ids(found: &[Found]) -> Vec<(&str, &str)> {
        found
            .iter()
            .map(|f| {
                let id = f.server.id.as_str();
                assert!(id.len() <= MAX_ID, "{id}");
                if f.server.source == "app" {
                    return (id, "app");
                }
                let (stem, hash) = id.rsplit_once('_').unwrap();
                assert!(
                    hash.len() == 8 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
                    "{id}"
                );
                (stem, f.server.source.as_str())
            })
            .collect()
    }

    #[test]
    fn apps_come_first_then_configs_deduplicated_and_loopback_only() {
        let (home, apps) = fixture();
        let found = discover_in(home.path(), &[apps.path().to_path_buf()]);
        assert_eq!(
            ids(&found),
            [
                ("xcode", "app"),
                ("figma", "app"),
                // Claude Desktop, sorted by name: "!!!" has no letters to keep.
                ("server", "Claude"),
                ("things_3", "Claude"),
                // Claude Code: "dup of things" and Claude Desktop's xcode are the same
                // command; the SSE, remote and project-scoped entries are skipped.
                ("local_http", "Claude Code"),
                ("things_3", "Claude Code"),
                // Cursor's file is broken and skipped. VS Code: the Figma URL is the
                // same server as the app's; LAN, lookalike and userinfo hosts are out.
                ("a_very_long_ser", "VS Code"),
                ("a_very_long_ser", "VS Code"),
                ("v6", "VS Code"),
            ]
        );
        let unique: HashSet<_> = found.iter().map(|f| &f.server.id).collect();
        assert_eq!(unique.len(), found.len());

        let xcode = &found[0].server;
        assert_eq!(xcode.app.as_deref(), Some("Xcode"));
        assert_eq!(xcode.bundle_id.as_deref(), Some("com.apple.dt.Xcode"));
        assert_eq!(xcode.detail, "xcrun mcpbridge");
        assert_eq!(xcode.fingerprint.len(), 64);
        assert_ne!(xcode.fingerprint, found[1].server.fingerprint);
        assert_eq!(found[1].server.transport, McpTransport::Http);

        let things = &found[3];
        assert_eq!(things.server.name, "Things 3");
        assert_eq!(things.server.detail, "npx -y things-mcp");
        assert!(
            matches!(&things.launch, Launch::Stdio { env, .. } if env["THINGS_TOKEN"] == "sekrit-1")
        );
        let shown =
            serde_json::to_string(&found.iter().map(|f| &f.server).collect::<Vec<_>>()).unwrap();
        assert!(!shown.contains("sekrit"), "{shown}");
    }

    #[test]
    fn ids_are_stable_between_runs_and_missing_files_are_fine() {
        let (home, apps) = fixture();
        let dirs = [apps.path().to_path_buf()];
        assert_eq!(
            discover_in(home.path(), &dirs),
            discover_in(home.path(), &dirs)
        );
        let empty = tempfile::tempdir().unwrap();
        assert!(discover_in(empty.path(), &[empty.path().join("Applications")]).is_empty());
    }

    #[test]
    fn slugs_fold_to_lowercase_words() {
        assert_eq!(slug("  Things 3 (beta)! "), "things_3_beta");
        assert_eq!(slug("\u{e9}t\u{e9}"), "t");
        assert_eq!(slug("__"), "server");
    }

    /// Every id seen in any run must always name the same command.
    fn record(history: &mut BTreeMap<String, Launch>, found: &[Found]) {
        for f in found {
            let mut launch = f.launch.clone();
            if let Launch::Stdio { env, .. } = &mut launch {
                env.clear();
            }
            let before = history.entry(f.server.id.clone()).or_insert(launch.clone());
            assert_eq!(
                *before, launch,
                "id {} moved to another server",
                f.server.id
            );
        }
    }

    #[test]
    fn an_id_never_moves_to_another_server_as_configs_change() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let none: [PathBuf; 0] = [];
        let desktop = "Library/Application Support/Claude/claude_desktop_config.json";
        let mut history = BTreeMap::new();
        let mut run = || {
            let found = discover_in(h, &none);
            record(&mut history, &found);
            found
                .into_iter()
                .map(|f| (f.server.id, f.server.detail, f.server.fingerprint))
                .collect::<Vec<_>>()
        };

        write(
            h,
            ".claude.json",
            &json!({"mcpServers": {"things": {"command": "things-a"}}}).to_string(),
        );
        let first = run();
        let original = first[0].0.clone();
        let approved = first[0].2.clone();
        assert_eq!(approved.len(), 64);

        // A clashing "things" in a file read earlier, then a second one in the same file.
        write(
            h,
            desktop,
            &json!({"mcpServers": {"things": {"command": "things-b"}}}).to_string(),
        );
        let second = run();
        assert_eq!(second.iter().filter(|s| s.0 == original).count(), 1);
        assert_eq!(second.len(), 2);
        write(
            h,
            ".claude.json",
            &json!({"mcpServers": {
                "Things": {"command": "things-c"},
                "things": {"command": "things-a", "env": {"NEW": "1"}}
            }})
            .to_string(),
        );
        // Only the environment changed: same id, but a new fingerprint, so the host
        // turns it off until the owner approves it again.
        let third = run();
        let same = third.iter().find(|s| s.0 == original).unwrap();
        assert_eq!(same.1, "things-a");
        assert_ne!(same.2, approved);

        // The original goes away, and a changed command is a new id.
        write(
            h,
            ".claude.json",
            &json!({"mcpServers": {"things": {"command": "things-a", "args": ["--v2"]}}})
                .to_string(),
        );
        let fourth = run();
        assert!(fourth.iter().all(|s| s.0 != original));
        std::fs::remove_file(h.join(".claude.json")).unwrap();
        run();
    }

    /// Collects what `tracing` writes while `f` runs.
    fn logs_of(f: impl FnOnce()) -> String {
        #[derive(Clone, Default)]
        struct Sink(std::sync::Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let sink = Sink::default();
        let writer = sink.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let bytes = sink.0.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn a_half_written_file_keeps_its_last_good_parse_and_logs_no_values() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let none: [PathBuf; 0] = [];
        let names = |found: Vec<Found>| -> Vec<String> {
            found.into_iter().map(|f| f.server.name).collect()
        };
        let good = json!({"mcpServers": {
            "kept": {"command": "kept-mcp"},
            "odd": {"command": "x", "args": "sk-proj-abcdef123456"}
        }})
        .to_string();
        write(h, ".cursor/mcp.json", &good);
        let logs = logs_of(|| assert_eq!(names(discover_in(h, &none)), ["kept"]));
        assert!(logs.contains("mcp.json") && logs.contains("Data"), "{logs}");
        assert!(!logs.contains("sk-proj"), "{logs}");

        // Cut off mid-write: the last good parse stands, and the log quotes nothing.
        write(h, ".cursor/mcp.json", &good[..good.len() / 2]);
        let logs = logs_of(|| assert_eq!(names(discover_in(h, &none)), ["kept"]));
        assert!(logs.contains("Eof") && !logs.contains("kept-mcp"), "{logs}");

        // An unchanged size and time is not read again.
        let path = h.join(".cursor/mcp.json");
        let same_size = json!({"mcpServers": {"next": {"command": "nextmcp"}}}).to_string();
        write(h, ".cursor/mcp.json", &same_size);
        assert_eq!(names(discover_in(h, &none)), ["next"]);
        let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();
        let swapped = same_size.replace("next", "swap");
        std::fs::write(&path, &swapped).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(stamp)
            .unwrap();
        assert_eq!(names(discover_in(h, &none)), ["next"]);

        std::fs::remove_file(&path).unwrap();
        assert!(discover_in(h, &none).is_empty());
    }
}
