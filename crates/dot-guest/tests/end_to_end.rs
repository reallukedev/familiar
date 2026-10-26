//! The daemon end to end over TCP with the real handshake, using only operations that
//! are safe on a developer's own Mac (no input, no apps, no clipboard, files in a
//! temporary folder only).
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "A setup or protocol error must fail the test"
)]
use dot_guest::config::Config;
use dot_guest::layout::Layout;
use dot_guest::ops::State;
use dot_guest::server;
use dot_wire::handshake::{self, HandshakeError};
use dot_wire::{
    Envelope, Exec, ExecResult, FilesOp, GuestError, GuestErrorCode, GuestInfo, Op, Reply,
    Screenshot, ScreenshotArgs, Script, ScriptLanguage, UiAction, UiActionArgs, UiTree, UiTreeArgs,
    UpdateOp,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

const INSTANCE: &str = "test-instance";

struct Guest {
    addr: SocketAddr,
    secret: Vec<u8>,
    home: tempfile::TempDir,
    state: Arc<State>,
    _stop: oneshot::Sender<()>,
}

async fn start() -> Guest {
    start_with(false).await
}

/// `supervised` as if started by `dot-guest supervise`.
async fn start_with(supervised: bool) -> Guest {
    let home = tempfile::tempdir().unwrap();
    let secret = dot_wire::random_bytes::<32>().to_vec();
    let config = Config {
        instance_id: INSTANCE.into(),
        secret: secret.clone(),
        port: 0,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut state = State::new(home.path().to_path_buf(), addr.port());
    state.supervised = supervised;
    let state = Arc::new(state);
    let (stop, stopped) = oneshot::channel::<()>();
    tokio::spawn(server::serve(listener, config, state.clone(), async {
        let _ = stopped.await;
    }));
    Guest {
        addr,
        secret,
        home,
        state,
        _stop: stop,
    }
}

struct Host {
    stream: TcpStream,
    next_id: u64,
    early: HashMap<u64, Reply>,
}

impl Host {
    async fn connect(guest: &Guest) -> (Self, GuestInfo) {
        let (host, info) = Self::try_connect(guest.addr, &guest.secret).await.unwrap();
        (host, info)
    }

    async fn try_connect(
        addr: SocketAddr,
        secret: &[u8],
    ) -> Result<(Self, GuestInfo), HandshakeError> {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let (_, info) = handshake::host(&mut stream, secret, INSTANCE).await?;
        Ok((
            Self {
                stream,
                next_id: 1,
                early: HashMap::new(),
            },
            info,
        ))
    }

    async fn send(&mut self, op: Op, deadline_ms: u64) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        dot_wire::frame::write(
            &mut self.stream,
            &Envelope::Call {
                id,
                deadline_ms,
                op,
            },
        )
        .await
        .unwrap();
        id
    }

    async fn cancel(&mut self, id: u64) {
        dot_wire::frame::write(&mut self.stream, &Envelope::Cancel { id })
            .await
            .unwrap();
    }

    async fn reply(&mut self, id: u64) -> Result<Value, GuestError> {
        if let Some(reply) = self.early.remove(&id) {
            return reply.result;
        }
        loop {
            let reply: Reply = tokio::time::timeout(
                Duration::from_secs(30),
                dot_wire::frame::read(&mut self.stream),
            )
            .await
            .expect("no reply within 30 s")
            .unwrap();
            if reply.id == id {
                return reply.result;
            }
            self.early.insert(reply.id, reply);
        }
    }

    async fn call(&mut self, op: Op, deadline_ms: u64) -> Result<Value, GuestError> {
        let id = self.send(op, deadline_ms).await;
        self.reply(id).await
    }
}

/// Point zsh at the temporary home for its startup files, so the developer's own
/// `~/.zprofile` (which may be slow or noisy) doesn't run inside the tests.
fn isolated(guest: &Guest, exec: Exec) -> Op {
    let mut exec = exec;
    exec.env.insert(
        "ZDOTDIR".into(),
        guest.home.path().to_string_lossy().into_owned(),
    );
    Op::Exec(exec)
}

fn exec(guest: &Guest, command: &str) -> Op {
    isolated(
        guest,
        Exec {
            command: command.into(),
            ..Exec::default()
        },
    )
}

async fn wait_for_pid(path: &Path) -> i32 {
    for _ in 0..500 {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(pid) = text.trim().parse()
        {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{} never appeared", path.display());
}

/// The process is gone (killed and reaped by launchd once orphaned).
async fn gone(pid: i32) -> bool {
    let pid = rustix::process::Pid::from_raw(pid).unwrap();
    for _ in 0..300 {
        if rustix::process::test_kill_process(pid).is_err() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

#[tokio::test]
async fn handshake_rejects_a_wrong_secret_and_then_serves_ping_and_info() {
    let guest = start().await;
    let wrong = Host::try_connect(guest.addr, b"not the secret").await;
    assert!(
        matches!(wrong, Err(HandshakeError::BadProof)),
        "wrong secret must fail"
    );

    let (mut host, hello_info) = Host::connect(&guest).await;
    assert!(!hello_info.os_version.is_empty());
    assert_eq!(host.call(Op::Ping, 1000).await.unwrap(), json!({}));

    let info: GuestInfo =
        serde_json::from_value(host.call(Op::Info, 20_000).await.unwrap()).unwrap();
    assert!(
        info.os_version.starts_with(|c: char| c.is_ascii_digit()),
        "{}",
        info.os_version
    );
    for op in ["exec", "screenshot", "ui_tree", "safari", "notify"] {
        assert!(info.capabilities.iter().any(|c| c == op), "missing {op}");
    }
    let status = std::fs::read_to_string(
        guest
            .home
            .path()
            .join("Library/Application Support/Dot Guest/status.json"),
    )
    .unwrap();
    assert!(status.contains("\"permissions\""));
}

#[tokio::test]
async fn exec_runs_in_zsh_with_cwd_env_and_stdin() {
    let guest = start().await;
    let (mut host, _) = Host::connect(&guest).await;
    std::fs::create_dir(guest.home.path().join("work")).unwrap();
    let op = isolated(
        &guest,
        Exec {
            command: "pwd; echo \"$GREETING\"; cat; exit 3".into(),
            cwd: Some("~/work".into()),
            env: [("GREETING".to_owned(), "hello".to_owned())].into(),
            stdin: Some("from stdin".into()),
            ..Exec::default()
        },
    );
    let result: ExecResult = serde_json::from_value(host.call(op, 10_000).await.unwrap()).unwrap();
    let work = std::fs::canonicalize(guest.home.path().join("work")).unwrap();
    assert_eq!(
        result.stdout,
        format!("{}\nhello\nfrom stdin", work.display())
    );
    assert_eq!(result.exit_code, Some(3));
    assert!(!result.timed_out && !result.truncated);
}

#[tokio::test]
async fn exec_deadline_kills_the_whole_process_group() {
    let guest = start().await;
    let (mut host, _) = Host::connect(&guest).await;
    let pidfile = guest.home.path().join("pid");
    let command = format!(
        "echo started; sleep 30 & echo $! > '{}'; wait",
        pidfile.display()
    );
    let started = Instant::now();
    let result: ExecResult =
        serde_json::from_value(host.call(exec(&guest, &command), 300).await.unwrap()).unwrap();
    assert!(result.timed_out);
    assert_eq!(
        result.stdout, "started\n",
        "output before the deadline is kept"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "took {:?}",
        started.elapsed()
    );
    let pid = wait_for_pid(&pidfile).await;
    assert!(
        gone(pid).await,
        "background sleep {pid} survived the deadline"
    );
}

#[tokio::test]
async fn cancel_kills_the_process_group_and_replies_cancelled() {
    let guest = start().await;
    let (mut host, _) = Host::connect(&guest).await;
    let pidfile = guest.home.path().join("pid");
    let command = format!("sleep 30 & echo $! > '{}'; wait", pidfile.display());
    let id = host.send(exec(&guest, &command), 60_000).await;
    let pid = wait_for_pid(&pidfile).await;
    host.cancel(id).await;
    let err = host.reply(id).await.unwrap_err();
    assert_eq!(err.code, GuestErrorCode::Cancelled);
    assert!(
        gone(pid).await,
        "background sleep {pid} survived cancellation"
    );
    assert_eq!(
        host.call(Op::Ping, 1000).await.unwrap(),
        json!({}),
        "connection still usable"
    );
}

#[tokio::test]
async fn calls_beyond_the_limit_are_busy_but_heartbeats_still_answer() {
    let guest = start().await;
    let (mut host, _) = Host::connect(&guest).await;
    let mut ids = Vec::new();
    for _ in 0..server::MAX_CONCURRENT_CALLS {
        ids.push(host.send(exec(&guest, "sleep 20"), 30_000).await);
    }
    let extra = host.call(exec(&guest, "echo hi"), 5_000).await.unwrap_err();
    assert_eq!(extra.code, GuestErrorCode::Busy);
    assert_eq!(host.call(Op::Ping, 1000).await.unwrap(), json!({}));
    for id in ids {
        host.cancel(id).await;
        assert_eq!(
            host.reply(id).await.unwrap_err().code,
            GuestErrorCode::Cancelled
        );
    }
    let after: ExecResult =
        serde_json::from_value(host.call(exec(&guest, "echo ok"), 5_000).await.unwrap()).unwrap();
    assert_eq!(
        after.stdout, "ok\n",
        "slots are released after cancellation"
    );
}

#[tokio::test]
async fn a_new_authenticated_connection_replaces_the_old_one() {
    let guest = start().await;
    let (mut first, _) = Host::connect(&guest).await;
    assert!(first.call(Op::Ping, 1000).await.is_ok());
    let (mut second, _) = Host::connect(&guest).await;
    let closed = tokio::time::timeout(
        Duration::from_secs(5),
        dot_wire::frame::read::<_, Reply>(&mut first.stream),
    )
    .await
    .expect("old connection was not closed");
    assert!(
        closed.is_err(),
        "old connection should be closed, got {closed:?}"
    );
    assert!(second.call(Op::Ping, 1000).await.is_ok());
}

#[tokio::test]
async fn unknown_operations_from_a_newer_host_are_unsupported_not_fatal() {
    let guest = start().await;
    let (mut host, _) = Host::connect(&guest).await;
    let raw = json!({ "kind": "call", "id": 77, "deadline_ms": 1000, "op": { "op": "teleport", "args": {} } });
    dot_wire::frame::write(&mut host.stream, &raw)
        .await
        .unwrap();
    let err = host.reply(77).await.unwrap_err();
    assert_eq!(err.code, GuestErrorCode::Unsupported);
    let bad_args = json!({ "kind": "call", "id": 78, "deadline_ms": 1000, "op": { "op": "exec", "args": { "cmd": 1 } } });
    dot_wire::frame::write(&mut host.stream, &bad_args)
        .await
        .unwrap();
    assert_eq!(
        host.reply(78).await.unwrap_err().code,
        GuestErrorCode::InvalidArgs
    );
    assert!(host.call(Op::Ping, 1000).await.is_ok());
}

#[tokio::test]
async fn files_round_trip_inside_a_temporary_folder() {
    let guest = start().await;
    let (mut host, _) = Host::connect(&guest).await;
    let root = guest.home.path().join("files");
    let p = |name: &str| root.join(name).to_string_lossy().into_owned();
    let files = |op: FilesOp| Op::Files(op);

    host.call(
        files(FilesOp::Mkdir {
            path: p("dir/nested"),
        }),
        5000,
    )
    .await
    .unwrap();
    let written = host
        .call(
            files(FilesOp::Write {
                path: p("dir/a.txt"),
                content: "h\u{e9}llo".into(),
                base64: false,
                append: false,
                create_only: true,
                parents: false,
            }),
            5000,
        )
        .await
        .unwrap();
    assert_eq!(written["size"], 6);
    host.call(
        files(FilesOp::Write {
            path: p("dir/a.txt"),
            content: " world".into(),
            base64: false,
            append: true,
            create_only: false,
            parents: false,
        }),
        5000,
    )
    .await
    .unwrap();
    let read = host
        .call(
            files(FilesOp::Read {
                path: p("dir/a.txt"),
                offset: 0,
                length: None,
            }),
            5000,
        )
        .await
        .unwrap();
    assert_eq!(read["content"], "h\u{e9}llo world");
    assert_eq!(read["base64"], false);
    assert_eq!(read["mime"], "text/plain");
    let partial = host
        .call(
            files(FilesOp::Read {
                path: p("dir/a.txt"),
                offset: 1,
                length: Some(1),
            }),
            5000,
        )
        .await
        .unwrap();
    assert_eq!(partial["truncated"], true);
    assert_eq!(
        partial["base64"], false,
        "a cut multi-byte character is trimmed, not base64"
    );

    let binary = host
        .call(
            files(FilesOp::Write {
                path: p("dir/bin.dat"),
                content: "AAH/".into(),
                base64: true,
                append: false,
                create_only: false,
                parents: false,
            }),
            5000,
        )
        .await
        .unwrap();
    assert_eq!(binary["size"], 3);
    let read = host
        .call(
            files(FilesOp::Read {
                path: p("dir/bin.dat"),
                offset: 0,
                length: None,
            }),
            5000,
        )
        .await
        .unwrap();
    assert_eq!(
        (read["content"].as_str(), read["base64"].as_bool()),
        (Some("AAH/"), Some(true))
    );

    let copied = host
        .call(
            files(FilesOp::Copy {
                from: p("dir"),
                to: p("copy"),
                overwrite: false,
            }),
            10_000,
        )
        .await
        .unwrap();
    assert_eq!(copied["kind"], "dir");
    let moved = host
        .call(
            files(FilesOp::Move {
                from: p("copy/a.txt"),
                to: p("moved.txt"),
                overwrite: false,
            }),
            5000,
        )
        .await
        .unwrap();
    assert_eq!(moved["kind"], "file");
    let clash = host
        .call(
            files(FilesOp::Copy {
                from: p("dir/a.txt"),
                to: p("moved.txt"),
                overwrite: false,
            }),
            5000,
        )
        .await
        .unwrap_err();
    assert_eq!(
        clash.code,
        GuestErrorCode::Failed,
        "existing destinations need overwrite"
    );

    let listing = host
        .call(
            files(FilesOp::List {
                path: root.to_string_lossy().into_owned(),
                depth: 1,
                hidden: false,
                limit: None,
            }),
            5000,
        )
        .await
        .unwrap();
    let names: Vec<String> = listing
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            e["path"]
                .as_str()
                .unwrap()
                .strip_prefix(&format!("{}/", root.display()))
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        names,
        [
            "copy",
            "copy/nested",
            "copy/bin.dat",
            "dir",
            "dir/nested",
            "dir/a.txt",
            "dir/bin.dat",
            "moved.txt"
        ]
    );

    let stat = host
        .call(
            files(FilesOp::Stat {
                path: p("moved.txt"),
            }),
            5000,
        )
        .await
        .unwrap();
    assert!(stat["modified"].as_str().unwrap().ends_with('Z'));
    let missing = host
        .call(files(FilesOp::Stat { path: p("nope") }), 5000)
        .await
        .unwrap_err();
    assert_eq!(missing.code, GuestErrorCode::NotFound);
    let relative = host
        .call(
            files(FilesOp::Stat {
                path: "relative/path".into(),
            }),
            5000,
        )
        .await
        .unwrap_err();
    assert_eq!(relative.code, GuestErrorCode::InvalidArgs);
}

#[tokio::test]
async fn scripts_pass_argv_and_map_errors() {
    let guest = start().await;
    let (mut host, _) = Host::connect(&guest).await;
    let script = |language, source: &str, args: Vec<String>| {
        Op::Script(Script {
            language,
            source: source.into(),
            args,
        })
    };
    let echo = host
        .call(
            script(
                ScriptLanguage::Javascript,
                "function run(argv){return JSON.stringify(JSON.parse(argv[0]))}",
                vec![r#"{"a":"\"quoted\" & é"}"#.into()],
            ),
            10_000,
        )
        .await
        .unwrap();
    assert_eq!(echo["output"], r#"{"a":"\"quoted\" & é"}"#);

    let missing = host
        .call(
            script(
                ScriptLanguage::Applescript,
                "on run argv\nerror \"Can't get that note.\" number -1728\nend run",
                vec![],
            ),
            10_000,
        )
        .await
        .unwrap_err();
    assert_eq!(missing.code, GuestErrorCode::NotFound);
    assert_eq!(missing.message, "Can't get that note.");

    let typed = host
        .call(
            script(
                ScriptLanguage::Javascript,
                "function run(){ throw new Error('INVALID_ARGS: no title') }",
                vec![],
            ),
            10_000,
        )
        .await
        .unwrap_err();
    assert_eq!(
        (typed.code, typed.message.as_str()),
        (GuestErrorCode::InvalidArgs, "no title")
    );

    let syntax = host
        .call(
            script(
                ScriptLanguage::Applescript,
                "on run\nreturn (1 +\nend run",
                vec![],
            ),
            10_000,
        )
        .await
        .unwrap_err();
    assert_eq!(syntax.code, GuestErrorCode::InvalidArgs);
}

#[tokio::test]
async fn ui_action_with_an_unknown_ref_is_stale_without_touching_the_ui() {
    let guest = start().await;
    let (mut host, _) = Host::connect(&guest).await;
    let op = Op::UiAction(UiActionArgs {
        reference: "e999".into(),
        action: UiAction::Press,
    });
    assert_eq!(
        host.call(op, 5000).await.unwrap_err().code,
        GuestErrorCode::StaleRef
    );
}

#[tokio::test]
#[ignore = "needs a logged-in GUI session (NSScreen is empty over SSH)"]
async fn info_lists_the_displays_of_a_logged_in_mac() {
    let guest = start().await;
    let (mut host, _) = Host::connect(&guest).await;
    let info: GuestInfo =
        serde_json::from_value(host.call(Op::Info, 20_000).await.unwrap()).unwrap();
    let main = info
        .displays
        .iter()
        .find(|d| d.main)
        .expect("a main display");
    assert_eq!(main.index, 1);
    assert!(main.width > 0 && main.scale >= 1.0);
}

#[tokio::test]
#[ignore = "needs a GUI session; captures the screen into memory (nothing is saved)"]
async fn screenshot_maps_pixels_to_points_or_names_the_missing_permission() {
    let guest = start().await;
    let (mut host, _) = Host::connect(&guest).await;
    let args = ScreenshotArgs {
        max_width: Some(800),
        ..ScreenshotArgs::default()
    };
    match host.call(Op::Screenshot(args), 20_000).await {
        Ok(value) => {
            let shot: Screenshot = serde_json::from_value(value).unwrap();
            assert_eq!(shot.mime, "image/jpeg");
            assert!(
                shot.width <= 800 && shot.height > 0,
                "{}x{}",
                shot.width,
                shot.height
            );
            assert!((shot.points_per_pixel - shot.area.width / f64::from(shot.width)).abs() < 1e-9);
            assert!(shot.data.len() > 1000);
        }
        Err(e) => {
            assert_eq!(e.code, GuestErrorCode::PermissionDenied, "{e:?}");
            assert!(e.hint.unwrap_or_default().contains("Screen"));
        }
    }
}

#[tokio::test]
#[ignore = "needs a GUI session; reads (never changes) Finder's accessibility tree"]
async fn finder_ui_tree_has_windows_first_and_unique_refs() {
    let guest = start().await;
    let (mut host, _) = Host::connect(&guest).await;
    let args = UiTreeArgs {
        app: Some("Finder".into()),
        max_depth: None,
        max_nodes: Some(200),
        include_menus: true,
    };
    match host.call(Op::UiTree(args), 20_000).await {
        Ok(value) => {
            let tree: UiTree = serde_json::from_value(value).unwrap();
            assert_eq!(tree.app, "Finder");
            assert!(tree.nodes.len() <= 200);
            let mut refs: Vec<&str> = tree.nodes.iter().map(|n| n.reference.as_str()).collect();
            refs.dedup();
            assert_eq!(refs.len(), tree.nodes.len());
            if let Some(first) = tree.nodes.first() {
                assert_eq!(first.depth, 1);
            }
        }
        Err(e) => {
            assert_eq!(e.code, GuestErrorCode::PermissionDenied, "{e:?}");
            assert!(e.hint.unwrap_or_default().contains("Accessibility"));
        }
    }
}

/// A stand-in agent binary: a script that answers `--version` like dot-guest does.
fn fake_agent(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\necho \"dot-guest {version}\"\n").into_bytes()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    dot_guest::util::hex(&sha2::Sha256::digest(bytes))
}

fn begin(version: &str, bytes: &[u8]) -> Op {
    Op::Update(UpdateOp::Begin {
        version: version.into(),
        sha256: sha256_hex(bytes),
        size: bytes.len() as u64,
    })
}

fn chunk(offset: usize, bytes: &[u8]) -> Op {
    use base64::Engine as _;
    Op::Update(UpdateOp::Chunk {
        offset: offset as u64,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// Install `version` the way `dot-guest install` leaves things.
fn seed_current(layout: &Layout, version: &str) {
    let bin = layout.version_binary(version);
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::write(&bin, fake_agent(version)).unwrap();
    layout.switch_to(version).unwrap();
}

fn partials(layout: &Layout) -> Vec<String> {
    std::fs::read_dir(layout.update_dir())
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_streamed_update_is_verified_installed_beside_the_old_one_and_restarts() {
    let guest = start_with(true).await;
    let layout = Layout::new(guest.home.path());
    seed_current(&layout, "1.0.0");
    let (mut host, hello) = Host::connect(&guest).await;
    assert!(hello.capabilities.iter().any(|c| c == "update"));

    let bytes = fake_agent("9.9.9-test");
    let (head, tail) = bytes.split_at(10);
    assert_eq!(
        host.call(begin("9.9.9-test", &bytes), 5000).await.unwrap(),
        json!({})
    );
    assert_eq!(
        host.call(chunk(0, head), 5000).await.unwrap(),
        json!({ "received": 10 })
    );
    assert_eq!(
        host.call(chunk(10, tail), 5000).await.unwrap(),
        json!({ "received": bytes.len() })
    );
    let reply = host
        .call(Op::Update(UpdateOp::Commit), 20_000)
        .await
        .unwrap();
    assert_eq!(
        reply,
        json!({ "installed": "9.9.9-test", "previous": "1.0.0", "restarting": true })
    );

    let bin = layout.version_binary("9.9.9-test");
    assert_eq!(std::fs::read(&bin).unwrap(), bytes);
    let mode =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&bin).unwrap().permissions());
    assert_eq!(mode & 0o777, 0o755);
    assert_eq!(layout.current_version().as_deref(), Some("9.9.9-test"));
    assert_eq!(layout.previous_version().as_deref(), Some("1.0.0"));
    let trial = layout.read_trial().unwrap();
    assert_eq!(trial.version, "9.9.9-test");
    assert_eq!(trial.sha256.as_deref(), Some(sha256_hex(&bytes).as_str()));
    assert_eq!(trial.attempts, 0);
    assert!(partials(&layout).is_empty(), "{:?}", partials(&layout));

    // The daemon is asked to exit so the supervisor starts the new version.
    tokio::time::timeout(Duration::from_secs(5), guest.state.restart.notified())
        .await
        .expect("the agent never asked to restart");
}

#[tokio::test]
async fn bad_updates_are_refused_and_install_nothing() {
    let guest = start().await;
    let layout = Layout::new(guest.home.path());
    let (mut host, hello) = Host::connect(&guest).await;
    assert!(
        !hello.capabilities.iter().any(|c| c == "update"),
        "an unsupervised agent can't restart into an update, so it must not offer it"
    );
    let code = |r: Result<Value, GuestError>| r.unwrap_err().code;
    let bytes = fake_agent("2.0.0");

    assert_eq!(
        code(host.call(chunk(0, &bytes), 5000).await),
        GuestErrorCode::InvalidArgs
    );
    assert_eq!(
        code(host.call(begin("../2.0.0", &bytes), 5000).await),
        GuestErrorCode::InvalidArgs
    );

    // Offsets must follow on exactly.
    host.call(begin("2.0.0", &bytes), 5000).await.unwrap();
    assert_eq!(
        code(host.call(chunk(1, &bytes[1..]), 5000).await),
        GuestErrorCode::InvalidArgs
    );
    host.call(chunk(0, &bytes[..4]), 5000).await.unwrap();
    assert_eq!(
        code(host.call(chunk(0, &bytes[..4]), 5000).await),
        GuestErrorCode::InvalidArgs
    );
    assert_eq!(
        code(
            host.call(chunk(4, &[bytes.clone(), vec![0]].concat()[4..]), 5000)
                .await
        ),
        GuestErrorCode::InvalidArgs,
        "past the announced size"
    );

    // Bytes that don't match the announced hash.
    let other = fake_agent("2.0.1");
    host.call(
        Op::Update(UpdateOp::Begin {
            version: "2.0.0".into(),
            sha256: sha256_hex(&bytes),
            size: other.len() as u64,
        }),
        5000,
    )
    .await
    .unwrap();
    host.call(chunk(0, &other), 5000).await.unwrap();
    let err = host
        .call(Op::Update(UpdateOp::Commit), 20_000)
        .await
        .unwrap_err();
    assert_eq!(err.code, GuestErrorCode::Failed);
    assert!(err.message.contains("SHA-256"), "{}", err.message);

    // A binary that reports a different version.
    host.call(begin("2.0.0", &other), 5000).await.unwrap();
    host.call(chunk(0, &other), 5000).await.unwrap();
    let err = host
        .call(Op::Update(UpdateOp::Commit), 20_000)
        .await
        .unwrap_err();
    assert_eq!(err.code, GuestErrorCode::Failed);
    assert!(err.message.contains("2.0.1"), "{}", err.message);

    // A binary that failed before.
    layout
        .mark_bad("2.0.0", Some(sha256_hex(&bytes)), "test")
        .unwrap();
    assert_eq!(
        code(host.call(begin("2.0.0", &bytes), 5000).await),
        GuestErrorCode::InvalidArgs
    );

    assert!(!layout.versions_dir().exists());
    assert!(!layout.current_link().exists());
    assert_eq!(layout.read_trial(), None);
    assert!(partials(&layout).is_empty(), "{:?}", partials(&layout));
}

#[tokio::test]
async fn abort_discards_the_transfer_and_an_unsupervised_commit_stays_up() {
    let guest = start().await;
    let layout = Layout::new(guest.home.path());
    let (mut host, _) = Host::connect(&guest).await;
    let bytes = fake_agent("3.0.0");

    host.call(begin("3.0.0", &bytes), 5000).await.unwrap();
    host.call(chunk(0, &bytes[..5]), 5000).await.unwrap();
    assert_eq!(partials(&layout), ["3.0.0.partial"]);
    assert_eq!(
        host.call(Op::Update(UpdateOp::Abort), 5000).await.unwrap(),
        json!({})
    );
    assert!(partials(&layout).is_empty());
    assert_eq!(
        host.call(Op::Update(UpdateOp::Commit), 5000)
            .await
            .unwrap_err()
            .code,
        GuestErrorCode::InvalidArgs
    );

    host.call(begin("3.0.0", &bytes), 5000).await.unwrap();
    host.call(chunk(0, &bytes), 5000).await.unwrap();
    let reply = host
        .call(Op::Update(UpdateOp::Commit), 20_000)
        .await
        .unwrap();
    assert_eq!(reply["installed"], "3.0.0");
    assert_eq!(reply["restarting"], false);
    assert!(
        reply["hint"]
            .as_str()
            .unwrap()
            .contains("dot-guest install")
    );
    assert_eq!(layout.current_version().as_deref(), Some("3.0.0"));
    assert!(
        tokio::time::timeout(Duration::from_secs(1), guest.state.restart.notified())
            .await
            .is_err(),
        "launchd would rerun the old binary, so an unsupervised agent must not exit"
    );
    assert_eq!(host.call(Op::Ping, 1000).await.unwrap(), json!({}));
}

#[tokio::test]
async fn the_first_handshake_ends_the_running_versions_trial() {
    let guest = start().await;
    let layout = Layout::new(guest.home.path());
    layout
        .write_trial(&dot_guest::layout::Trial {
            version: dot_guest::VERSION.into(),
            sha256: None,
            started_at: 0,
            attempts: 2,
        })
        .unwrap();
    let (mut host, _) = Host::connect(&guest).await;
    host.call(Op::Ping, 1000).await.unwrap();
    assert_eq!(layout.read_trial(), None);
}
