//! Encryption at rest through the API: recovery keys at setup, unlocking after a
//! restart, password changes that keep the data key, and reset links that need the
//! recovery key (or erase the encrypted Macs).
#![allow(clippy::unwrap_used, clippy::expect_used)]
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const ORIGIN: &str = "http://localhost:8470";
const FIRST: &str = "a long passphrase";
const SECOND: &str = "an even longer passphrase";
const THIRD: &str = "the third passphrase here";

#[derive(Clone)]
struct Client {
    router: Router,
    cookie: Option<String>,
    csrf: Option<String>,
}

struct Res {
    status: StatusCode,
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
        if let Some(sc) = res
            .headers()
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
        Res { status, json }
    }
}

/// An authenticator. Each code must be for a later 30-second step than the last one
/// used, and the server accepts one step of drift, so this waits only when it must.
struct Totp {
    secret: Vec<u8>,
    last: i64,
}

impl Totp {
    async fn code(&mut self) -> String {
        loop {
            let current = dot_home::db::now() / 30;
            let step = (self.last + 1).max(current - 1);
            if step <= current + 1 {
                self.last = step;
                return dot_home::auth::totp::code(&self.secret, step * 30);
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
}

/// Sign in with a password and an authenticator code.
async fn sign_in(c: &mut Client, password: &str, totp: &mut Totp) -> Res {
    let first = c
        .send(
            "POST",
            "/api/login",
            Some(json!({"username": "alex", "password": password})),
        )
        .await;
    assert_eq!(
        first.json["error"]["code"], "totp_required",
        "{}",
        first.json
    );
    let challenge = first.json["challenge"].as_str().unwrap().to_owned();
    let code = totp.code().await;
    c.send(
        "POST",
        "/api/login/totp",
        Some(json!({"challenge": challenge, "code": code})),
    )
    .await
}

async fn confirm(c: &mut Client, password: &str, totp: &mut Totp) -> Res {
    let code = totp.code().await;
    c.send(
        "POST",
        "/api/account/confirm",
        Some(json!({"password": password, "code": code})),
    )
    .await
}

async fn reset_link(admin: &mut Client, uid: &str) -> String {
    let r = admin
        .send("POST", &format!("/api/admin/users/{uid}/reset"), None)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.json);
    let url = r.json["invite_url"].as_str().unwrap();
    url.rsplit('/').next().unwrap().to_owned()
}

#[tokio::test]
async fn keys_follow_the_password_and_the_recovery_key() {
    let dir = tempfile::tempdir().unwrap();
    let mut config: dot_home::config::Config =
        toml::from_str(&format!("public_url = '{ORIGIN}'")).unwrap();
    config.data_dir = dir.path().to_path_buf();
    // This test never fills a disk; don't let the build machine's free space decide.
    config.limits.min_free_disk_gb = 0;
    std::fs::create_dir_all(dir.path().join("images")).unwrap();
    // New Macs need a base image to exist; nothing here starts one.
    std::fs::write(dir.path().join("images/base.qcow2"), b"").unwrap();
    let app = dot_home::build(config).await.unwrap();
    let router = dot_home::router(app.clone());
    let mut c = Client {
        router,
        cookie: None,
        csrf: None,
    };

    // Setup shows a recovery key once; the session asks for it to be saved.
    let url = std::fs::read_to_string(dir.path().join("setup-url.txt")).unwrap();
    let token = url.split("token=").nth(1).unwrap().to_owned();
    let setup = c
        .send(
            "POST",
            "/api/setup",
            Some(json!({"token": token, "username": "alex", "display_name": "Alex", "password": FIRST})),
        )
        .await;
    assert_eq!(setup.status, StatusCode::OK, "{}", setup.json);
    let first_key = setup.json["recovery_key"].as_str().unwrap().to_owned();
    assert_eq!(first_key.split('-').count(), 8);
    assert_eq!(setup.json["locked"], false);
    assert_eq!(setup.json["recovery_key_needed"], true);
    let uid = setup.json["user"]["id"].as_str().unwrap().to_owned();
    c.send("POST", "/api/account/recovery-key/saved", None)
        .await;
    let s = c.send("GET", "/api/session", None).await;
    assert_eq!(s.json["recovery_key_needed"], false);
    assert!(s.json.get("recovery_key").is_none(), "shown only once");

    // Nothing plaintext reaches the database.
    let db = dot_home::db::Db::open(&dir.path().join("dot-home.db")).unwrap();
    let rows: i64 = db
        .call(|c| c.query_row("SELECT COUNT(*) FROM user_keys", [], |r| r.get(0)))
        .await
        .unwrap();
    assert_eq!(rows, 1);

    // Two-factor, as required by default.
    let begin = c.send("POST", "/api/account/totp", None).await;
    let mut totp = Totp {
        secret: data_encoding::BASE32_NOPAD
            .decode(begin.json["secret"].as_str().unwrap().as_bytes())
            .unwrap(),
        last: 0,
    };
    let enroll = totp.code().await;
    let ok = c
        .send(
            "POST",
            "/api/account/totp/confirm",
            Some(json!({"code": enroll})),
        )
        .await;
    assert_eq!(ok.json["totp_enabled"], true, "{}", ok.json);

    // A new Mac on this server is encrypted from the start.
    let created = c
        .send(
            "POST",
            "/api/instances",
            Some(json!({"name": "Studio", "driver": "qemu", "cpus": 2, "memory_mb": 4096, "disk_gb": 64})),
        )
        .await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.json);
    let mac = created.json["instance"].clone();
    let iid = mac["id"].as_str().unwrap().to_owned();
    assert_eq!(mac["encrypted"], true);
    assert_eq!(mac["locked"], false);
    assert_eq!(mac["can_encrypt"], false);

    // A restart forgets every key: the Mac is locked until Alex signs in again.
    app.fleet.keys().clear();
    assert_eq!(
        c.send("GET", "/api/session", None).await.json["locked"],
        true
    );
    let view = c.send("GET", &format!("/api/instances/{iid}"), None).await;
    assert_eq!(view.json["locked"], true);
    let start = c
        .send(
            "POST",
            &format!("/api/instances/{iid}/power"),
            Some(json!({"action": "start"})),
        )
        .await;
    assert_eq!(start.status, StatusCode::LOCKED);
    assert_eq!(start.json["error"]["code"], "locked");

    // The password alone doesn't unlock: the key waits for the authenticator code.
    c.send("POST", "/api/logout", None).await;
    let first = c
        .send(
            "POST",
            "/api/login",
            Some(json!({"username": "alex", "password": FIRST})),
        )
        .await;
    assert_eq!(first.json["error"]["code"], "totp_required");
    assert!(!app.fleet.keys().is_unlocked(&uid));
    let challenge = first.json["challenge"].as_str().unwrap().to_owned();
    let code = totp.code().await;
    let signed = c
        .send(
            "POST",
            "/api/login/totp",
            Some(json!({"challenge": challenge, "code": code})),
        )
        .await;
    assert_eq!(signed.status, StatusCode::OK, "{}", signed.json);
    assert_eq!(signed.json["locked"], false);
    assert!(signed.json.get("recovery_key").is_none());
    let view = c.send("GET", &format!("/api/instances/{iid}"), None).await;
    assert_eq!(view.json["locked"], false);

    // Confirming the password also unlocks (the portal's Unlock).
    app.fleet.keys().clear();
    let wrong = c
        .send(
            "POST",
            "/api/account/confirm",
            Some(json!({"password": "not my password", "code": "000000"})),
        )
        .await;
    assert_eq!(wrong.status, StatusCode::BAD_REQUEST);
    assert!(!app.fleet.keys().is_unlocked(&uid));
    let unlocked = confirm(&mut c, FIRST, &mut totp).await;
    assert_eq!(unlocked.status, StatusCode::OK, "{}", unlocked.json);
    assert!(app.fleet.keys().is_unlocked(&uid));
    let key_before = app.fleet.keys().get(&uid).unwrap();

    // Changing the password rewraps the same key: the new one opens it, the old doesn't.
    let changed = c
        .send(
            "POST",
            "/api/account/password",
            Some(json!({"current": FIRST, "password": SECOND})),
        )
        .await;
    assert_eq!(changed.status, StatusCode::OK, "{}", changed.json);
    let stored = dot_home::keys::load(&db, &uid).await.unwrap().unwrap();
    let opened = tokio::task::spawn_blocking({
        let (s, u) = (stored.clone(), uid.clone());
        move || s.open_with_password(&u, SECOND)
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(format!("{opened:?}"), "DataKey(..)");
    assert!(
        tokio::task::spawn_blocking({
            let (s, u) = (stored.clone(), uid.clone());
            move || s.open_with_password(&u, FIRST).is_none()
        })
        .await
        .unwrap()
    );
    app.fleet.keys().clear();
    assert_eq!(
        confirm(&mut c, SECOND, &mut totp).await.status,
        StatusCode::OK
    );
    drop(key_before);

    // A new recovery key replaces the old one.
    let fresh = c.send("POST", "/api/account/recovery-key", None).await;
    assert_eq!(fresh.status, StatusCode::OK, "{}", fresh.json);
    let second_key = fresh.json["recovery_key"].as_str().unwrap().to_owned();
    assert_ne!(second_key, first_key);
    assert_eq!(
        c.send("GET", "/api/session", None).await.json["recovery_key_needed"],
        true,
        "a new key must be saved again"
    );

    // A reset link: the new password can't open the key, so it asks for the recovery key.
    let link = reset_link(&mut c, &uid).await;
    let mut visitor = Client {
        router: c.router.clone(),
        cookie: None,
        csrf: None,
    };
    let info = visitor
        .send("GET", &format!("/api/invites/{link}"), None)
        .await;
    assert_eq!(info.json["needs_recovery_key"], true);
    assert_eq!(info.json["encrypted_macs"], 1);
    let accept = |body: Value| {
        let mut v = visitor.clone();
        let link = link.clone();
        async move {
            v.send("POST", &format!("/api/invites/{link}/accept"), Some(body))
                .await
        }
    };
    let missing = accept(json!({"password": THIRD})).await;
    assert_eq!(missing.status, StatusCode::CONFLICT);
    assert_eq!(missing.json["error"]["code"], "recovery_key_required");
    let old = accept(json!({"password": THIRD, "recovery_key": first_key})).await;
    assert_eq!(
        old.json["error"]["field"], "recovery_key",
        "the replaced key is dead"
    );
    let typo = accept(json!({"password": THIRD, "recovery_key": "ABCD"})).await;
    assert_eq!(typo.json["error"]["field"], "recovery_key");
    // Lowercase, spaces instead of dashes: still read.
    let typed = second_key.to_lowercase().replace('-', " ");
    let kept = accept(json!({"password": THIRD, "recovery_key": typed})).await;
    assert_eq!(kept.json["error"]["code"], "totp_required", "{}", kept.json);
    let view = c.send("GET", &format!("/api/instances/{iid}"), None).await;
    assert_eq!(
        view.status,
        StatusCode::UNAUTHORIZED,
        "the reset ended every session"
    );
    let back = sign_in(&mut c, THIRD, &mut totp).await;
    assert_eq!(back.status, StatusCode::OK, "{}", back.json);
    assert!(app.fleet.keys().is_unlocked(&uid));
    let view = c.send("GET", &format!("/api/instances/{iid}"), None).await;
    assert_eq!(view.json["locked"], false, "the Mac survived the reset");
    let events = c.send("GET", "/api/account/security", None).await;
    assert!(
        events
            .json
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "recovery_key_used")
    );

    // Another reset, without the recovery key: the encrypted Mac is erased on request.
    let link = reset_link(&mut c, &uid).await;
    let mut visitor = Client {
        router: c.router.clone(),
        cookie: None,
        csrf: None,
    };
    let erased = visitor
        .send(
            "POST",
            &format!("/api/invites/{link}/accept"),
            Some(json!({"password": FIRST, "discard_encrypted_macs": true})),
        )
        .await;
    assert_eq!(
        erased.json["error"]["code"], "totp_required",
        "{}",
        erased.json
    );
    // The reset's own code step carries the new key (and its recovery key) through.
    let challenge = erased.json["challenge"].as_str().unwrap().to_owned();
    let code = totp.code().await;
    let back = visitor
        .send(
            "POST",
            "/api/login/totp",
            Some(json!({"challenge": challenge, "code": code})),
        )
        .await;
    assert_eq!(back.status, StatusCode::OK, "{}", back.json);
    assert!(app.fleet.keys().is_unlocked(&uid));
    c = visitor;
    let shown = back.json["recovery_key"].as_str().unwrap();
    assert_ne!(shown, second_key, "a new key, with a new recovery key");
    assert_eq!(
        c.send("GET", &format!("/api/instances/{iid}"), None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(c.send("GET", "/api/instances", None).await.json, json!([]));
    let events = c.send("GET", "/api/account/security", None).await;
    assert!(
        events
            .json
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "keys_reset")
    );
}
