//! End-to-end: setup, two-factor, CSRF, a remote Mac with a fake agent, OAuth for an
//! assistant, MCP tool calls through permissions and approvals, token rotation, invites
//! and isolation between people.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use dot_wire::{Envelope, GuestErrorCode, GuestInfo, McpOp, Op, Reply, frame};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::time::Duration;
use tower::ServiceExt;

const ORIGIN: &str = "http://localhost:8470";

struct Client {
    router: Router,
    cookie: Option<String>,
    csrf: Option<String>,
}

struct Res {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    json: Value,
}

impl Client {
    async fn send(&mut self, method: &str, path: &str, body: Option<Value>) -> Res {
        let mut b = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "localhost:8470")
            .header(header::ORIGIN, ORIGIN);
        if let Some(c) = &self.cookie {
            b = b.header(header::COOKIE, c);
        }
        if let Some(t) = &self.csrf {
            b = b.header("x-dot-csrf", t);
        }
        let req = match body {
            Some(v) => b
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(v.to_string())),
            None => b.body(Body::empty()),
        }
        .unwrap();
        let res = self.router.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        if let Some(sc) = headers
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
        {
            let pair = sc.split(';').next().unwrap().to_owned();
            self.cookie = if pair.ends_with('=') {
                None
            } else {
                Some(pair)
            };
        }
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if let Some(c) = json.get("csrf").and_then(Value::as_str) {
            self.csrf = Some(c.to_owned());
        }
        Res {
            status,
            headers,
            json,
        }
    }

    async fn form(&self, path: &str, form: &[(&str, &str)]) -> Res {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form)
            .finish();
        let req = Request::builder()
            .method("POST")
            .uri(path)
            .header(header::HOST, "localhost:8470")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap();
        let res = self.router.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        Res {
            status,
            headers,
            json: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }
}

/// What the fake Mac's Xcode server currently runs, as its fingerprint.
static XCODE_FINGERPRINT: std::sync::Mutex<&str> = std::sync::Mutex::new("v1");
/// How many times the fake Mac was asked for an app server's tools.
static TOOLS_LISTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A fake dot-guest: answers the ops these tests use.
async fn fake_guest(listener: tokio::net::TcpListener, instance_id: String, secret: Vec<u8>) {
    loop {
        let Ok((mut s, _)) = listener.accept().await else {
            return;
        };
        let (id, secret) = (instance_id.clone(), secret.clone());
        tokio::spawn(async move {
            let info = || GuestInfo {
                os_version: "26.0".into(),
                hostname: "test-mac".into(),
                user: "alex".into(),
                capabilities: vec!["mcp".into()],
                ..Default::default()
            };
            if dot_wire::handshake::guest(&mut s, &secret, &id, "test", info)
                .await
                .is_err()
            {
                return;
            }
            let (mut r, mut w) = tokio::io::split(s);
            while let Ok(Envelope::Call { id, op, .. }) = frame::read(&mut r).await {
                let result = match op {
                    Op::Ping => Ok(json!({})),
                    Op::Exec(e) => Ok(
                        json!({"exit_code": 0, "stdout": format!("ran: {}", e.command), "stderr": "", "truncated": false, "timed_out": false, "duration_ms": 3}),
                    ),
                    Op::Script(s) => {
                        Ok(json!({"output": s.args.first().cloned().unwrap_or_default()}))
                    }
                    Op::Screenshot(_) => Ok(
                        json!({"data": "aGk=", "mime": "image/jpeg", "width": 1440, "height": 900,
                        "area": {"x": 0.0, "y": 0.0, "width": 1440.0, "height": 900.0}, "points_per_pixel": 1.0}),
                    ),
                    Op::Pointer(_) => Ok(json!({})),
                    Op::Mcp(McpOp::Discover) => Ok(json!([{
                        "id": "xcode", "name": "Xcode", "app": "Xcode", "bundle_id": "com.apple.dt.Xcode",
                        "transport": "stdio", "detail": "xcrun mcpbridge", "source": "app",
                        "fingerprint": *XCODE_FINGERPRINT.lock().unwrap()
                    }])),
                    Op::Mcp(McpOp::ListTools { .. }) => {
                        TOOLS_LISTED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Ok(json!([
                            { "name": "BuildProject", "description": "Builds the open project.", "input_schema": { "type": "object", "properties": { "scheme": { "type": "string" } } }, "read_only": false, "destructive": false },
                            { "name": "get.log", "title": "Build Log", "input_schema": { "type": "object" }, "read_only": true, "destructive": false }
                        ]))
                    }
                    Op::Mcp(McpOp::CallTool {
                        tool, arguments, ..
                    }) => Ok(
                        json!({ "content": [{ "type": "text", "text": format!("{tool} {}", Value::Object(arguments)) }], "isError": false }),
                    ),
                    _ => Err(dot_wire::GuestError::new(
                        GuestErrorCode::Unsupported,
                        "not in the fake",
                    )),
                };
                if frame::write(&mut w, &Reply { id, result }).await.is_err() {
                    return;
                }
            }
        });
    }
}

async fn mcp(
    client: &Client,
    token: &str,
    session: Option<&str>,
    body: Value,
) -> (StatusCode, Option<String>, Value) {
    let mut b = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header(header::HOST, "localhost:8470")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header("mcp-protocol-version", "2025-06-18");
    if let Some(s) = session {
        b = b.header("mcp-session-id", s);
    }
    let res = client
        .router
        .clone()
        .oneshot(b.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let sid = res
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    // Streamable HTTP may answer as a single SSE event.
    let json_text = text
        .lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .rfind(|d| !d.trim().is_empty())
        .unwrap_or(&text);
    let v = serde_json::from_str(json_text)
        .unwrap_or_else(|_| json!({ "raw": text, "status": status.as_u16() }));
    (status, sid, v)
}

async fn call_tool(client: &Client, token: &str, sid: &str, name: &str, args: Value) -> Value {
    let (status, _, v) = mcp(client, token, Some(sid), json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":name,"arguments":args}})).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    v["result"].clone()
}

fn error_code(result: &Value) -> Option<String> {
    let text = result["content"][0]["text"].as_str()?;
    serde_json::from_str::<Value>(text).ok()?["error_code"]
        .as_str()
        .map(str::to_owned)
}

#[tokio::test]
async fn the_whole_flow() {
    let dir = tempfile::tempdir().unwrap();
    let mut config: dot_home::config::Config =
        toml::from_str(&format!("public_url = '{ORIGIN}'")).unwrap();
    config.data_dir = dir.path().to_path_buf();
    let app = dot_home::build(config).await.unwrap();
    let router = dot_home::router(app.clone());
    let mut owner = Client {
        router: router.clone(),
        cookie: None,
        csrf: None,
    };

    // First run: setup needs the token from the server.
    let s = owner.send("GET", "/api/session", None).await;
    assert_eq!(s.json["setup_required"], true);
    assert!(s.headers.get("content-security-policy").is_some());
    let url = std::fs::read_to_string(dir.path().join("setup-url.txt")).unwrap();
    let token = url.split("token=").nth(1).unwrap().to_owned();
    let bad = owner.send("POST", "/api/setup", Some(json!({"token":"nope","username":"alex","display_name":"Alex","password":"a long passphrase"}))).await;
    assert_eq!(bad.status, StatusCode::FORBIDDEN);
    let ok = owner.send("POST", "/api/setup", Some(json!({"token":token,"username":"alex","display_name":"Alex","password":"a long passphrase"}))).await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.json);
    assert_eq!(ok.json["user"]["role"], "admin");
    assert!(owner.cookie.as_deref().unwrap().starts_with("dot="));

    // Two-factor is required by default before anything else.
    let blocked = owner.send("GET", "/api/instances", None).await;
    assert_eq!(blocked.json["error"]["code"], "totp_setup_required");
    let begin = owner.send("POST", "/api/account/totp", None).await;
    let alex_totp = data_encoding::BASE32_NOPAD
        .decode(begin.json["secret"].as_str().unwrap().as_bytes())
        .unwrap();
    let code = dot_home::auth::totp::code(&alex_totp, dot_home::db::now());
    let confirmed = owner
        .send(
            "POST",
            "/api/account/totp/confirm",
            Some(json!({"code": code})),
        )
        .await;
    assert_eq!(confirmed.json["totp_enabled"], true, "{}", confirmed.json);
    assert_eq!(
        owner.send("GET", "/api/instances", None).await.json,
        json!([])
    );

    // CSRF: a change without the header is refused.
    let csrf = owner.csrf.take();
    let no_csrf = owner
        .send(
            "POST",
            "/api/instances",
            Some(json!({"name":"x","driver":"remote","remote_host":"127.0.0.1:1"})),
        )
        .await;
    assert_eq!(no_csrf.json["error"]["code"], "csrf");
    owner.csrf = csrf;

    // A remote Mac with a fake agent.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let public = owner
        .send(
            "POST",
            "/api/instances",
            Some(json!({"name":"Studio","driver":"remote","remote_host":"8.8.8.8:7466"})),
        )
        .await;
    assert_eq!(
        public.json["error"]["field"], "remote_host",
        "public addresses are refused"
    );
    let created = owner
        .send(
            "POST",
            "/api/instances",
            Some(json!({"name":"Studio","driver":"remote","remote_host": addr.to_string()})),
        )
        .await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.json);
    let iid = created.json["instance"]["id"].as_str().unwrap().to_owned();
    let pairing = created.json["pairing"].as_str().unwrap();
    let parts: Vec<&str> = pairing.split(':').collect();
    assert_eq!(&parts[..3], ["dotpair", "v1", iid.as_str()]);
    assert_eq!(parts[4], addr.port().to_string());
    let guest_secret = URL_SAFE_NO_PAD.decode(parts[3]).unwrap();
    tokio::spawn(fake_guest(listener, iid.clone(), guest_secret));
    let mut ready = false;
    for _ in 0..50 {
        if owner
            .send("GET", &format!("/api/instances/{iid}"), None)
            .await
            .json["state"]
            == "ready"
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(ready, "the Mac never became ready");

    // OAuth: register, authorize, consent, token.
    let reg = owner.send("POST", "/oauth/register", Some(json!({"client_name":"Claude","redirect_uris":["https://claude.ai/api/mcp/auth_callback"]}))).await;
    assert_eq!(reg.status, StatusCode::CREATED);
    let client_id = reg.json["client_id"].as_str().unwrap().to_owned();
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
    let evil = owner
        .send("GET", &format!("/oauth/authorize?response_type=code&client_id={client_id}&redirect_uri=https://evil.example/cb&code_challenge={challenge}&code_challenge_method=S256"), None)
        .await;
    assert_eq!(
        evil.status,
        StatusCode::BAD_REQUEST,
        "never redirect to an unregistered URI"
    );
    let auth = owner
        .send("GET", &format!("/oauth/authorize?response_type=code&client_id={client_id}&redirect_uri=https://claude.ai/api/mcp/auth_callback&state=xyz&code_challenge={challenge}&code_challenge_method=S256"), None)
        .await;
    let location = auth
        .headers
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let request_id = location
        .strip_prefix("/connect?request=")
        .unwrap()
        .to_owned();
    let info = owner
        .send("GET", &format!("/api/oauth/requests/{request_id}"), None)
        .await;
    assert_eq!(info.json["client_name"], "Claude");
    let decided = owner
        .send(
            "POST",
            &format!("/api/oauth/requests/{request_id}"),
            Some(json!({"instance_id": iid, "approve": true})),
        )
        .await;
    let redirect = url::Url::parse(decided.json["redirect"].as_str().unwrap()).unwrap();
    let q: std::collections::HashMap<_, _> = redirect.query_pairs().into_owned().collect();
    assert_eq!(q["state"], "xyz");
    let code = q["code"].clone();
    let wrong = owner
        .form(
            "/oauth/token",
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", "https://claude.ai/api/mcp/auth_callback"),
                ("client_id", &client_id),
                ("code_verifier", "x".repeat(43).as_str()),
            ],
        )
        .await;
    assert_eq!(
        wrong.json["error"], "invalid_grant",
        "a wrong verifier burns the code"
    );
    // The code was used by the failed attempt, so start a fresh authorization.
    let auth = owner.send("GET", &format!("/oauth/authorize?response_type=code&client_id={client_id}&redirect_uri=https://claude.ai/api/mcp/auth_callback&code_challenge={challenge}&code_challenge_method=S256"), None).await;
    let request_id = auth
        .headers
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .strip_prefix("/connect?request=")
        .unwrap()
        .to_owned();
    let decided = owner
        .send(
            "POST",
            &format!("/api/oauth/requests/{request_id}"),
            Some(json!({"instance_id": iid, "approve": true})),
        )
        .await;
    let redirect = url::Url::parse(decided.json["redirect"].as_str().unwrap()).unwrap();
    let code: String = redirect
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();
    let tok = owner
        .form(
            "/oauth/token",
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", "https://claude.ai/api/mcp/auth_callback"),
                ("client_id", &client_id),
                ("code_verifier", verifier),
            ],
        )
        .await;
    assert_eq!(tok.status, StatusCode::OK, "{}", tok.json);
    assert_eq!(tok.headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
    let access = tok.json["access_token"].as_str().unwrap().to_owned();
    let refresh = tok.json["refresh_token"].as_str().unwrap().to_owned();

    // MCP without a token is a 401 that points at the metadata.
    let (st, _, _) = mcp(&owner, "dot_at_wrong", None, json!({})).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    let (st, sid, init) = mcp(&owner, &access, None, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).await;
    assert_eq!(st, StatusCode::OK, "{init}");
    let sid = sid.unwrap();
    let _ = mcp(
        &owner,
        &access,
        Some(&sid),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await;
    let (_, _, tools) = mcp(
        &owner,
        &access,
        Some(&sid),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )
    .await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("{tools}"))
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for expected in [
        "mac_screenshot",
        "safari_screenshot",
        "mac_shell",
        "calendar_events",
        "mail_send",
    ] {
        assert!(names.contains(&expected), "missing {expected}");
    }

    // get_owner_feedback is always offered.
    assert!(names.contains(&"get_owner_feedback"));

    // New Macs ask before changes; Allow Everything lets the assistant work unattended.
    let mode = owner
        .send(
            "GET",
            &format!("/api/instances/{iid}/permissions/mode"),
            None,
        )
        .await;
    assert_eq!(mode.json["mode"], "changes", "{}", mode.json);
    let all = owner
        .send(
            "PUT",
            &format!("/api/instances/{iid}/permissions/mode"),
            Some(json!({"mode": "all"})),
        )
        .await;
    assert_eq!(all.json["mode"], "all", "{}", all.json);
    assert!(
        all.json["groups"]
            .as_array()
            .unwrap()
            .iter()
            .all(|g| g["mode"] == "allow"),
        "{}",
        all.json
    );

    let shot = call_tool(&owner, &access, &sid, "mac_screenshot", json!({})).await;
    assert_eq!(shot["content"][0]["type"], "image");
    let click = call_tool(
        &owner,
        &access,
        &sid,
        "mac_click",
        json!({"x": 10, "y": 20}),
    )
    .await;
    assert_eq!(
        click["structuredContent"]["clicked_points"],
        json!([10.0, 20.0])
    );
    let ran = call_tool(
        &owner,
        &access,
        &sid,
        "mac_shell",
        json!({"command": "echo hi"}),
    )
    .await;
    assert_eq!(ran["structuredContent"]["stdout"], "ran: echo hi");
    let invalid = call_tool(&owner, &access, &sid, "mac_shell", json!({"command": 5})).await;
    assert_eq!(error_code(&invalid).as_deref(), Some("INVALID_ARGS"));

    // The click and the command ran without asking, so they wait in Review; the
    // screenshot only read and the invalid call never ran.
    let pending = owner
        .send("GET", &format!("/api/instances/{iid}/review"), None)
        .await;
    let items = pending
        .json
        .as_array()
        .unwrap_or_else(|| panic!("{}", pending.json));
    let tools_in: Vec<&str> = items.iter().map(|i| i["tool"].as_str().unwrap()).collect();
    assert_eq!(tools_in, ["mac_shell", "mac_click"], "{}", pending.json);
    assert_eq!(items[0]["session"], sid.as_str());
    assert!(items[0]["activity_id"].is_string());
    let mac = owner
        .send("GET", &format!("/api/instances/{iid}"), None)
        .await;
    assert_eq!(mac.json["counts"]["pending_review"], 2);
    let (shell_item, click_item) = (
        items[0]["id"].as_str().unwrap().to_owned(),
        items[1]["id"].as_str().unwrap().to_owned(),
    );
    let declined = owner
        .send(
            "POST",
            &format!("/api/instances/{iid}/review/{shell_item}"),
            Some(json!({"decision": "decline", "note": "Don't echo in my shell."})),
        )
        .await;
    assert_eq!(declined.json["status"], "declined", "{}", declined.json);
    let again = owner
        .send(
            "POST",
            &format!("/api/instances/{iid}/review/{shell_item}"),
            Some(json!({"decision": "accept"})),
        )
        .await;
    assert_eq!(again.status, StatusCode::CONFLICT);
    let accepted = owner
        .send(
            "POST",
            &format!("/api/instances/{iid}/review/accept-all"),
            Some(json!({})),
        )
        .await;
    assert_eq!(accepted.json["accepted"], 1, "{}", accepted.json);
    let done = owner
        .send(
            "GET",
            &format!("/api/instances/{iid}/review?status=done"),
            None,
        )
        .await;
    assert_eq!(done.json.as_array().unwrap().len(), 2);
    assert!(
        done.json
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["id"] == click_item.as_str() && i["status"] == "accepted")
    );

    // The decline reaches the assistant once, in front of its next result.
    let next = call_tool(&owner, &access, &sid, "mac_screenshot", json!({})).await;
    let notice = next["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        notice.contains("declined your earlier change")
            && notice.contains("Don't echo in my shell."),
        "{next}"
    );
    assert_eq!(next["content"][1]["type"], "image");
    let after = call_tool(&owner, &access, &sid, "mac_screenshot", json!({})).await;
    assert_eq!(after["content"][0]["type"], "image", "delivered only once");
    let feedback = call_tool(&owner, &access, &sid, "get_owner_feedback", json!({})).await;
    assert_eq!(feedback["structuredContent"]["declined"], json!([]));

    // App MCP servers: found without starting anything, and off until the owner turns
    // one on, even under Allow Everything.
    let found = owner
        .send(
            "POST",
            &format!("/api/instances/{iid}/app-servers/refresh"),
            Some(json!({})),
        )
        .await;
    let xcode = found
        .json
        .as_array()
        .unwrap_or_else(|| panic!("{}", found.json))
        .iter()
        .find(|g| g["id"] == "mcp:xcode")
        .cloned()
        .unwrap_or_else(|| panic!("{}", found.json));
    assert_eq!(xcode["mode"], "deny");
    assert_eq!(xcode["server"]["detail"], "xcrun mcpbridge");
    assert_eq!(TOOLS_LISTED.load(std::sync::atomic::Ordering::SeqCst), 0);
    let listed = |v: &Value| -> Vec<String> {
        v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let (_, _, tools) = mcp(
        &owner,
        &access,
        Some(&sid),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}),
    )
    .await;
    assert!(!listed(&tools).iter().any(|n| n.starts_with("mcp_")));

    let on = owner
        .send(
            "PUT",
            &format!("/api/instances/{iid}/permissions/mcp:xcode"),
            Some(json!({"mode": "allow"})),
        )
        .await;
    assert_eq!(on.json["mode"], "allow", "{}", on.json);
    let bridged: Vec<&str> = on.json["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(bridged.len(), 2, "{}", on.json);
    assert_eq!(bridged[0], "mcp_xcode_BuildProject");
    let log_tool = bridged[1].to_owned();
    assert!(log_tool.starts_with("mcp_xcode_get_log_"), "{log_tool}");
    // Turning a server on by hand doesn't make the Mac's approval mode Custom.
    let mode = owner
        .send(
            "GET",
            &format!("/api/instances/{iid}/permissions/mode"),
            None,
        )
        .await;
    assert_eq!(mode.json["mode"], "all", "{}", mode.json);

    let (_, _, tools) = mcp(
        &owner,
        &access,
        Some(&sid),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/list"}),
    )
    .await;
    let build = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "mcp_xcode_BuildProject")
        .cloned()
        .unwrap_or_else(|| panic!("{tools}"));
    assert_eq!(build["annotations"]["readOnlyHint"], false);
    assert!(
        build["description"]
            .as_str()
            .unwrap()
            .ends_with("From Xcode\u{2019}s MCP server on the Mac.")
    );
    let built = call_tool(
        &owner,
        &access,
        &sid,
        "mcp_xcode_BuildProject",
        json!({"scheme": "App"}),
    )
    .await;
    assert_eq!(
        built["content"][0]["text"], r#"BuildProject {"scheme":"App"}"#,
        "{built}"
    );
    let log = call_tool(&owner, &access, &sid, &log_tool, json!({})).await;
    assert_eq!(
        log["content"][0]["text"], "get.log {}",
        "the server gets its own tool name back"
    );
    let bad = call_tool(
        &owner,
        &access,
        &sid,
        "mcp_xcode_BuildProject",
        json!({"scheme": 3}),
    )
    .await;
    assert_eq!(
        error_code(&bad).as_deref(),
        Some("INVALID_ARGS"),
        "the server's schema is checked first"
    );

    // Ask for Everything makes an allowed server ask; switching back never raises it.
    let xcode_in = |groups: &Value| {
        groups
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["id"] == "mcp:xcode")
            .cloned()
            .unwrap_or_else(|| panic!("{groups}"))
    };
    for (mode, expected) in [("ask", "ask"), ("all", "ask")] {
        let set = owner
            .send(
                "PUT",
                &format!("/api/instances/{iid}/permissions/mode"),
                Some(json!({"mode": mode})),
            )
            .await;
        assert_eq!(
            xcode_in(&set.json["groups"])["mode"],
            expected,
            "{}",
            set.json
        );
    }

    // A server whose command or environment changed is off until it's turned on again.
    owner
        .send(
            "PUT",
            &format!("/api/instances/{iid}/permissions/mcp:xcode"),
            Some(json!({"mode": "allow"})),
        )
        .await;
    *XCODE_FINGERPRINT.lock().unwrap() = "v2";
    let looked = owner
        .send(
            "POST",
            &format!("/api/instances/{iid}/app-servers/refresh"),
            Some(json!({})),
        )
        .await;
    let changed = xcode_in(&looked.json);
    assert_eq!(changed["mode"], "deny", "{changed}");
    assert_eq!(changed["server"]["changed"], true);
    let stale = call_tool(&owner, &access, &sid, "mcp_xcode_BuildProject", json!({})).await;
    assert_eq!(error_code(&stale).as_deref(), Some("DENIED"), "{stale}");
    let again = owner
        .send(
            "PUT",
            &format!("/api/instances/{iid}/permissions/mcp:xcode"),
            Some(json!({"mode": "allow"})),
        )
        .await;
    assert_eq!(again.json["mode"], "allow", "{}", again.json);
    assert_eq!(again.json["server"]["changed"], false);

    let off = owner
        .send(
            "PUT",
            &format!("/api/instances/{iid}/permissions/mcp:xcode"),
            Some(json!({"mode": "deny"})),
        )
        .await;
    assert_eq!(off.json["mode"], "deny");
    // Off is refused as off, even with arguments that wouldn't pass.
    let refused = call_tool(
        &owner,
        &access,
        &sid,
        "mcp_xcode_BuildProject",
        json!({"scheme": 3}),
    )
    .await;
    assert_eq!(error_code(&refused).as_deref(), Some("DENIED"), "{refused}");
    let junk = owner
        .send(
            "PUT",
            &format!("/api/instances/{iid}/permissions/mcp:Not-Valid"),
            Some(json!({"mode": "allow"})),
        )
        .await;
    assert_eq!(junk.status, StatusCode::NOT_FOUND);

    // Ask first: the call waits for the owner's approval.
    owner
        .send(
            "PUT",
            &format!("/api/instances/{iid}/permissions/shell"),
            Some(json!({"mode":"ask"})),
        )
        .await;
    let (router2, access2, sid2) = (owner.router.clone(), access.clone(), sid.clone());
    let waiting = tokio::spawn(async move {
        let c = Client {
            router: router2,
            cookie: None,
            csrf: None,
        };
        call_tool(
            &c,
            &access2,
            &sid2,
            "mac_shell",
            json!({"command": "brew install jq"}),
        )
        .await
    });
    let mut approval = Value::Null;
    for _ in 0..50 {
        let list = owner
            .send("GET", &format!("/api/instances/{iid}/approvals"), None)
            .await;
        if let Some(a) = list
            .json
            .as_array()
            .and_then(|a| a.first())
            .filter(|a| a["status"] == "pending")
        {
            approval = a.clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        approval["summary"]
            .as_str()
            .unwrap()
            .contains("brew install jq")
    );
    assert_eq!(
        approval["arguments"]["command"], "brew install jq",
        "the owner sees every argument"
    );
    let aid = approval["id"].as_str().unwrap();
    owner
        .send(
            "POST",
            &format!("/api/instances/{iid}/approvals/{aid}"),
            Some(json!({"decision":"approve"})),
        )
        .await;
    let approved = waiting.await.unwrap();
    assert_eq!(
        approved["structuredContent"]["stdout"],
        "ran: brew install jq"
    );

    // Sending needs approval by default (outgoing = ask), and deny hides tools.
    owner
        .send(
            "PUT",
            &format!("/api/instances/{iid}/permissions/shell"),
            Some(json!({"mode":"deny"})),
        )
        .await;
    let (_, _, tools) = mcp(
        &owner,
        &access,
        Some(&sid),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}),
    )
    .await;
    assert!(
        !tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "mac_shell")
    );
    let denied = call_tool(&owner, &access, &sid, "mac_shell", json!({"command": "ls"})).await;
    assert_eq!(error_code(&denied).as_deref(), Some("DENIED"));

    // Pause stops everything.
    owner
        .send(
            "PATCH",
            &format!("/api/instances/{iid}"),
            Some(json!({"paused": true})),
        )
        .await;
    let paused = call_tool(&owner, &access, &sid, "mac_screenshot", json!({})).await;
    assert_eq!(error_code(&paused).as_deref(), Some("PAUSED"));
    owner
        .send(
            "PATCH",
            &format!("/api/instances/{iid}"),
            Some(json!({"paused": false})),
        )
        .await;

    // Snapshot retention and cap are Mac settings; 7 days and no limit by default.
    let mac = owner
        .send("GET", &format!("/api/instances/{iid}"), None)
        .await;
    assert_eq!(mac.json["snapshot_keep_days"], 7);
    assert_eq!(mac.json["snapshot_max_gb"], 0);
    let mac = owner
        .send(
            "PATCH",
            &format!("/api/instances/{iid}"),
            Some(json!({"snapshot_keep_days": 0, "snapshot_max_gb": 50})),
        )
        .await;
    assert_eq!(mac.json["snapshot_keep_days"], 0);
    assert_eq!(mac.json["snapshot_max_gb"], 50);
    let bad = owner
        .send(
            "PATCH",
            &format!("/api/instances/{iid}"),
            Some(json!({"snapshot_keep_days": 3651})),
        )
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    let usage = owner
        .send("GET", &format!("/api/instances/{iid}/snapshot-usage"), None)
        .await;
    assert_eq!(usage.json["limit_bytes"], 50_000_000_000u64);
    assert_eq!(usage.json["kept_over_limit"], false);
    let missing = owner
        .send(
            "PUT",
            &format!("/api/instances/{iid}/snapshots/nope"),
            Some(json!({"kept": true})),
        )
        .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);

    // Activity was recorded without content.
    let activity = owner
        .send("GET", &format!("/api/instances/{iid}/activity"), None)
        .await;
    let items = activity.json["items"].as_array().unwrap();
    assert!(
        items
            .iter()
            .any(|i| i["tool"] == "mac_shell" && i["outcome"] == "denied")
    );
    assert!(
        items
            .iter()
            .any(|i| i["tool"] == "mac_shell" && i["detail"] == "brew")
    );

    // Refresh tokens rotate; reusing an old one revokes the whole grant.
    let r1 = owner
        .form(
            "/oauth/token",
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &refresh),
                ("client_id", &client_id),
            ],
        )
        .await;
    assert_eq!(r1.status, StatusCode::OK);
    let new_access = r1.json["access_token"].as_str().unwrap().to_owned();
    let reuse = owner
        .form(
            "/oauth/token",
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &refresh),
                ("client_id", &client_id),
            ],
        )
        .await;
    assert_eq!(reuse.json["error"], "invalid_grant");
    let (st, _, _) = mcp(
        &owner,
        &new_access,
        Some(&sid),
        json!({"jsonrpc":"2.0","id":9,"method":"tools/list"}),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "reuse revoked the grant");

    // Updates: a local build can't verify signed updates, and says so (CI builds carry the
    // update key, so they can); per-Mac channels save.
    let up = owner.send("GET", "/api/admin/updates", None).await;
    if dot_home::updates::disabled_reason().is_some() {
        assert_eq!(up.json["enabled"], false);
        assert!(
            up.json["disabled_reason"]
                .as_str()
                .unwrap()
                .contains("release pipeline")
        );
    } else {
        assert_eq!(up.json["enabled"], true);
        assert!(up.json["disabled_reason"].is_null());
    }
    // A server starts on the channel it was built from (CI builds nightly), else stable.
    assert_eq!(
        up.json["settings"]["channel"],
        dot_home::updates::build::channel().unwrap_or("stable")
    );
    let mac = owner
        .send(
            "PATCH",
            &format!("/api/instances/{iid}"),
            Some(json!({"agent_channel": "nightly"})),
        )
        .await;
    assert_eq!(mac.json["agent"]["channel"], "nightly");
    assert_eq!(mac.json["agent"]["version"], "test");
    let bad = owner
        .send(
            "PATCH",
            &format!("/api/instances/{iid}"),
            Some(json!({"agent_channel": "beta"})),
        )
        .await;
    assert_eq!(bad.json["error"]["field"], "agent_channel");
    let set = owner
        .send(
            "PUT",
            "/api/admin/updates",
            Some(json!({"channel": "nightly", "window_start_hour": 4})),
        )
        .await;
    assert_eq!(set.json["settings"]["channel"], "nightly");
    assert_eq!(set.json["settings"]["window_start_hour"], 4);

    // Personal tokens work for Claude Code.
    let pat = owner
        .send(
            "POST",
            &format!("/api/instances/{iid}/tokens"),
            Some(json!({"name":"Claude Code"})),
        )
        .await;
    let secret = pat.json["secret"].as_str().unwrap().to_owned();
    assert!(secret.starts_with("dot_pat_"));
    let (st, _, _) = mcp(&owner, &secret, None, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"cc","version":"1"}}})).await;
    assert_eq!(st, StatusCode::OK);

    // A second person joins by invite and can't see the first person's Mac.
    let inv = owner
        .send("POST", "/api/admin/invites", Some(json!({"role":"member"})))
        .await;
    let code = inv.json["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let mut sam = Client {
        router: router.clone(),
        cookie: None,
        csrf: None,
    };
    let joined = sam
        .send(
            "POST",
            &format!("/api/invites/{code}/accept"),
            Some(json!({"username":"sam","display_name":"Sam","password":"another long phrase"})),
        )
        .await;
    assert_eq!(joined.status, StatusCode::OK, "{}", joined.json);
    let again = Client {
        router: router.clone(),
        cookie: None,
        csrf: None,
    }
    .send(
        "POST",
        &format!("/api/invites/{code}/accept"),
        Some(json!({"username":"sam2","display_name":"Sam","password":"another long phrase"})),
    )
    .await;
    assert_eq!(
        again.status,
        StatusCode::NOT_FOUND,
        "invites are single use"
    );
    // Sam must enroll two-factor first, then still can't see Alex's Mac.
    let begin = sam.send("POST", "/api/account/totp", None).await;
    let s2 = data_encoding::BASE32_NOPAD
        .decode(begin.json["secret"].as_str().unwrap().as_bytes())
        .unwrap();
    sam.send(
        "POST",
        "/api/account/totp/confirm",
        Some(json!({"code": dot_home::auth::totp::code(&s2, dot_home::db::now())})),
    )
    .await;
    assert_eq!(
        sam.send("GET", "/api/instances", None).await.json,
        json!([])
    );
    assert_eq!(
        sam.send("GET", &format!("/api/instances/{iid}"), None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        sam.send("GET", "/api/admin/users", None).await.status,
        StatusCode::FORBIDDEN
    );

    // Wrong passwords lock the account after five tries.
    let mut stranger = Client {
        router: router.clone(),
        cookie: None,
        csrf: None,
    };
    for _ in 0..4 {
        let r = stranger
            .send(
                "POST",
                "/api/login",
                Some(json!({"username":"sam","password":"wrong password!"})),
            )
            .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
    }
    let locked = stranger
        .send(
            "POST",
            "/api/login",
            Some(json!({"username":"sam","password":"wrong password!"})),
        )
        .await;
    assert_eq!(locked.json["error"]["code"], "rate_limited");
    let even_right = stranger
        .send(
            "POST",
            "/api/login",
            Some(json!({"username":"sam","password":"another long phrase"})),
        )
        .await;
    assert_eq!(even_right.json["error"]["code"], "rate_limited");

    // A reset link (even one an admin makes) changes the password but still needs the code.
    let sam_id = owner
        .send("GET", "/api/admin/users", None)
        .await
        .json
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == "sam")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let reset = owner
        .send("POST", &format!("/api/admin/users/{sam_id}/reset"), None)
        .await;
    let rcode = reset.json["invite_url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let mut taker = Client {
        router: router.clone(),
        cookie: None,
        csrf: None,
    };
    let took = taker
        .send(
            "POST",
            &format!("/api/invites/{rcode}/accept"),
            Some(json!({"password":"admin chose this one"})),
        )
        .await;
    assert_eq!(took.json["error"]["code"], "totp_required", "{}", took.json);
    assert!(
        taker.cookie.is_none(),
        "no session without the second factor"
    );

    // Signing in with two-factor.
    let mut alex = Client {
        router,
        cookie: None,
        csrf: None,
    };
    let step1 = alex
        .send(
            "POST",
            "/api/login",
            Some(json!({"username":"ALEX","password":"a long passphrase"})),
        )
        .await;
    assert_eq!(step1.json["error"]["code"], "totp_required");
    let challenge = step1.json["challenge"].as_str().unwrap().to_owned();
    let reused = alex.send("POST", "/api/login/totp", Some(json!({"challenge": challenge, "code": dot_home::auth::totp::code(&alex_totp, dot_home::db::now())}))).await;
    assert_eq!(
        reused.json["error"]["field"], "code",
        "the enrollment code can't be replayed"
    );
    // One step ahead is within the allowed drift and hasn't been used.
    let fresh = dot_home::auth::totp::code(&alex_totp, dot_home::db::now() + 30);
    let signed = alex
        .send(
            "POST",
            "/api/login/totp",
            Some(json!({"challenge": challenge, "code": fresh})),
        )
        .await;
    assert_eq!(signed.status, StatusCode::OK, "{}", signed.json);
    assert_eq!(signed.json["user"]["username"], "alex");
    let log = alex.send("GET", "/api/account/security", None).await;
    let kinds: Vec<&str> = log
        .json
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert!(
        kinds.contains(&"sign_in")
            && kinds.contains(&"totp_enabled")
            && kinds.contains(&"grant_created")
    );
}
