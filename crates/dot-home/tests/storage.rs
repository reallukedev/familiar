//! Storage: what people see, the limits admins set, discarding saved memory, and new
//! Macs refused over a limit (docs/home/API.md "Storage").
#![allow(clippy::unwrap_used, clippy::expect_used)]
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use dot_home::state::App;
use dot_wire::{Envelope, GuestInfo, Op, Reply, frame};
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

impl Client {
    fn new(router: &Router) -> Self {
        Self {
            router: router.clone(),
            cookie: None,
            csrf: None,
        }
    }

    async fn send(&mut self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
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
            self.cookie = Some(sc.split(';').next().unwrap().to_owned());
        }
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if let Some(c) = json.get("csrf").and_then(Value::as_str) {
            self.csrf = Some(c.to_owned());
        }
        (status, json)
    }

    /// Enroll two-factor, which every account needs before anything else.
    async fn enroll_totp(&mut self) {
        let (_, begin) = self.send("POST", "/api/account/totp", None).await;
        let secret = data_encoding::BASE32_NOPAD
            .decode(begin["secret"].as_str().unwrap().as_bytes())
            .unwrap();
        let code = dot_home::auth::totp::code(&secret, dot_home::db::now());
        let (status, body) = self
            .send(
                "POST",
                "/api/account/totp/confirm",
                Some(json!({"code": code})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
}

struct Server {
    _dir: tempfile::TempDir,
    app: App,
    /// Alex, the admin.
    owner: Client,
    /// Sam, a member.
    sam: Client,
    sam_id: String,
}

async fn server() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let mut config: dot_home::config::Config =
        toml::from_str(&format!("public_url = '{ORIGIN}'")).unwrap();
    config.data_dir = dir.path().to_path_buf();
    let app = dot_home::build(config).await.unwrap();
    let router = dot_home::router(app.clone());

    let mut owner = Client::new(&router);
    let url = std::fs::read_to_string(dir.path().join("setup-url.txt")).unwrap();
    let token = url.split("token=").nth(1).unwrap().to_owned();
    let (status, _) = owner.send("POST", "/api/setup", Some(json!({"token":token,"username":"alex","display_name":"Alex","password":"a long passphrase"}))).await;
    assert_eq!(status, StatusCode::OK);
    owner.enroll_totp().await;

    let (_, invite) = owner
        .send("POST", "/api/admin/invites", Some(json!({"role":"member"})))
        .await;
    let code = invite["url"].as_str().unwrap().rsplit('/').next().unwrap();
    let mut sam = Client::new(&router);
    let (status, joined) = sam
        .send(
            "POST",
            &format!("/api/invites/{code}/accept"),
            Some(json!({"username":"sam","display_name":"Sam","password":"another long phrase"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{joined}");
    sam.enroll_totp().await;
    let sam_id = user_id(&app, "sam").await;

    let s = Server {
        _dir: dir,
        app,
        owner,
        sam,
        sam_id,
    };
    s.confirm("alex", true).await;
    s
}

async fn user_id(app: &App, username: &'static str) -> String {
    app.db
        .call(move |c| {
            c.query_row(
                "SELECT id FROM users WHERE username = ?1",
                [username],
                |r| r.get(0),
            )
        })
        .await
        .unwrap()
}

impl Server {
    /// Set or end the "confirmed your password recently" window for someone's sessions,
    /// as `POST /api/account/confirm` would (without spending a two-factor code).
    async fn confirm(&self, username: &'static str, confirmed: bool) {
        let until = if confirmed {
            dot_home::db::now() + 600
        } else {
            0
        };
        self.app
            .db
            .call(move |c| {
                c.execute(
                    "UPDATE sessions SET confirmed_until = ?2 WHERE user_id = (SELECT id FROM users WHERE username = ?1)",
                    rusqlite::params![username, until],
                )
            })
            .await
            .unwrap();
    }

    /// One person's row in `AdminStorage`.
    fn person<'a>(admin: &'a Value, id: &str) -> &'a Value {
        admin["people"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["user_id"] == id)
            .unwrap()
    }

    async fn new_qemu_mac(&mut self, name: &str, disk_gb: u32) -> (StatusCode, Value) {
        self.owner
            .send(
                "POST",
                "/api/instances",
                Some(json!({"name": name, "driver": "qemu", "cpus": 2, "memory_mb": 4096, "disk_gb": disk_gb})),
            )
            .await
    }

    async fn state(&mut self, id: &str) -> Value {
        self.owner
            .send("GET", &format!("/api/instances/{id}"), None)
            .await
            .1["state"]
            .clone()
    }

    async fn wait_for(&mut self, id: &str, state: &str) {
        for _ in 0..50 {
            if self.state(id).await == state {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("the Mac never became {state}: {}", self.state(id).await);
    }
}

/// A fake dot-guest that only answers pings, enough for a remote Mac to be ready.
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
                    _ => Err(dot_wire::GuestError::new(
                        dot_wire::GuestErrorCode::Unsupported,
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

#[tokio::test]
async fn people_see_their_storage_and_admins_set_limits() {
    let mut s = server().await;
    let sam_id = s.sam_id.clone();

    // Everyone sees their own storage, with the server's defaults until an admin changes them.
    let (status, mine) = s.sam.send("GET", "/api/storage", None).await;
    assert_eq!(status, StatusCode::OK, "{mine}");
    assert_eq!(
        mine["limits"],
        json!({"quota_gb": 0, "max_disk_gb": 512, "max_snapshots": 0})
    );
    assert_eq!(
        mine["sources"],
        json!({"quota_gb": "default", "max_disk_gb": "default", "max_snapshots": "default"})
    );
    assert_eq!(
        (&mine["used_bytes"], &mine["over_quota"], &mine["macs"]),
        (&json!(0), &json!(false), &json!([]))
    );
    assert!(mine["measured_at"].is_string());

    // Members can't see or change the server's storage, even after confirming their password.
    s.confirm("sam", true).await;
    let (status, _) = s.sam.send("GET", "/api/admin/storage", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = s
        .sam
        .send(
            "PUT",
            "/api/admin/storage/defaults",
            Some(json!({"quota_gb": 1})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = s
        .sam
        .send(
            "PUT",
            &format!("/api/admin/users/{sam_id}/storage"),
            Some(json!({"quota_gb": 10_000})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Admins see everyone.
    let (status, admin) = s.owner.send("GET", "/api/admin/storage", None).await;
    assert_eq!(status, StatusCode::OK, "{admin}");
    assert_eq!(admin["people"].as_array().unwrap().len(), 2);
    assert_eq!(
        admin["defaults"],
        json!({"quota_gb": 0, "max_disk_gb": 512, "max_snapshots": 0, "default_disk_gb": 128,
               "reserve_gb": 20, "snapshot_total_gb": 0})
    );
    assert_eq!(
        Server::person(&admin, &sam_id)["overrides"],
        json!({"quota_gb": null, "max_disk_gb": null, "max_snapshots": null})
    );
    assert!(admin["disk_total_bytes"].as_u64().unwrap() > 0);

    // Changing limits needs a recent password confirmation.
    s.confirm("alex", false).await;
    for (path, body) in [
        (
            "/api/admin/storage/defaults".to_owned(),
            json!({"quota_gb": 300}),
        ),
        (
            format!("/api/admin/users/{sam_id}/storage"),
            json!({"quota_gb": 50}),
        ),
    ] {
        let (status, err) = s.owner.send("PUT", &path, Some(body)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        assert_eq!(err["error"]["code"], "reauth_required", "{path}");
    }
    s.confirm("alex", true).await;

    // Out of range: the field and a sentence, and nothing is saved.
    for (body, field) in [
        (json!({"quota_gb": 100_001}), "quota_gb"),
        (json!({"max_disk_gb": 63}), "max_disk_gb"),
        (json!({"max_disk_gb": 4097}), "max_disk_gb"),
        (json!({"default_disk_gb": 32}), "default_disk_gb"),
        (json!({"max_snapshots": 101}), "max_snapshots"),
        (json!({"reserve_gb": 4}), "reserve_gb"),
        (json!({"reserve_gb": 10_001}), "reserve_gb"),
        (json!({"snapshot_total_gb": -1}), "snapshot_total_gb"),
        (json!({"quota_gb": "300"}), "quota_gb"),
        (json!({"quota_gb": 1.5}), "quota_gb"),
        (json!({"quota_gb": null}), "quota_gb"),
        // The default disk can't be bigger than the largest allowed (512 now)...
        (json!({"default_disk_gb": 1024}), "default_disk_gb"),
        // ...nor the largest smaller than the default (128 now).
        (json!({"max_disk_gb": 64}), "max_disk_gb"),
        // A valid key alongside a bad one doesn't sneak through.
        (json!({"quota_gb": 300, "reserve_gb": 1}), "reserve_gb"),
    ] {
        let (status, err) = s
            .owner
            .send("PUT", "/api/admin/storage/defaults", Some(body.clone()))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {err}");
        assert_eq!(err["error"]["code"], "invalid", "{body}");
        assert_eq!(err["error"]["field"], field, "{body}: {err}");
        assert!(!err["error"]["message"].as_str().unwrap().is_empty());
    }
    let (_, admin) = s.owner.send("GET", "/api/admin/storage", None).await;
    assert_eq!(admin["defaults"]["quota_gb"], 0, "nothing was saved");
    assert_eq!(admin["defaults"]["reserve_gb"], 20);

    // Saving defaults: only the keys sent change, and the reserve applies at once.
    let (status, admin) = s
        .owner
        .send(
            "PUT",
            "/api/admin/storage/defaults",
            Some(
                json!({"quota_gb": 300, "max_disk_gb": 1024, "default_disk_gb": 1024,
                        "reserve_gb": 5, "snapshot_total_gb": 2000}),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{admin}");
    assert_eq!(
        admin["defaults"],
        json!({"quota_gb": 300, "max_disk_gb": 1024, "max_snapshots": 0, "default_disk_gb": 1024,
               "reserve_gb": 5, "snapshot_total_gb": 2000})
    );
    assert_eq!(s.app.fleet.reserve_gb().await, 5);
    assert_eq!(dot_home::storage::snapshot_total_gb(&s.app).await, 2000);
    let (status, admin) = s
        .owner
        .send(
            "PUT",
            "/api/admin/storage/defaults",
            Some(json!({"max_snapshots": 10, "quota_gb": 0})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{admin}");
    assert_eq!(
        (
            &admin["defaults"]["max_snapshots"],
            &admin["defaults"]["quota_gb"],
            &admin["defaults"]["max_disk_gb"]
        ),
        (&json!(10), &json!(0), &json!(1024))
    );

    // Overrides for one person: a number sets one, a missing key is left alone...
    let (status, admin) = s
        .owner
        .send(
            "PUT",
            &format!("/api/admin/users/{sam_id}/storage"),
            Some(json!({"quota_gb": 50, "max_snapshots": 0})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{admin}");
    let sam = Server::person(&admin, &sam_id);
    assert_eq!(
        sam["overrides"],
        json!({"quota_gb": 50, "max_disk_gb": null, "max_snapshots": 0})
    );
    assert_eq!(
        sam["limits"],
        json!({"quota_gb": 50, "max_disk_gb": 1024, "max_snapshots": 0})
    );
    let (_, mine) = s.sam.send("GET", "/api/storage", None).await;
    assert_eq!(
        mine["sources"],
        json!({"quota_gb": "override", "max_disk_gb": "default", "max_snapshots": "override"})
    );
    // ...and null clears one back to the default.
    let (status, admin) = s
        .owner
        .send(
            "PUT",
            &format!("/api/admin/users/{sam_id}/storage"),
            Some(json!({"quota_gb": null, "max_disk_gb": 256})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{admin}");
    let sam = Server::person(&admin, &sam_id);
    assert_eq!(
        sam["overrides"],
        json!({"quota_gb": null, "max_disk_gb": 256, "max_snapshots": 0})
    );
    assert_eq!(sam["limits"]["quota_gb"], 0, "back to the default");
    let (_, mine) = s.sam.send("GET", "/api/storage", None).await;
    assert_eq!(
        mine["sources"],
        json!({"quota_gb": "default", "max_disk_gb": "override", "max_snapshots": "override"})
    );

    // Overrides are checked against the same ranges, and only for people who exist.
    for (body, field) in [
        (json!({"max_disk_gb": 10}), "max_disk_gb"),
        (json!({"quota_gb": 100_001}), "quota_gb"),
        (json!({"max_snapshots": -1}), "max_snapshots"),
        (json!({"max_snapshots": "5"}), "max_snapshots"),
    ] {
        let (status, err) = s
            .owner
            .send(
                "PUT",
                &format!("/api/admin/users/{sam_id}/storage"),
                Some(body.clone()),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {err}");
        assert_eq!(err["error"]["field"], field, "{body}");
    }
    let (status, _) = s
        .owner
        .send(
            "PUT",
            "/api/admin/users/nobody/storage",
            Some(json!({"quota_gb": 5})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, admin) = s.owner.send("GET", "/api/admin/storage", None).await;
    assert_eq!(
        Server::person(&admin, &sam_id)["overrides"],
        json!({"quota_gb": null, "max_disk_gb": 256, "max_snapshots": 0}),
        "refused changes saved nothing"
    );
}

#[tokio::test]
async fn saved_memory_is_discarded_and_new_macs_respect_limits() {
    let mut s = server().await;
    let owner_id = user_id(&s.app, "alex").await;
    // An image to build Macs from (never booted here), and a reserve this machine can meet.
    std::fs::write(s.app.config.data_dir.join("images/base.qcow2"), b"").unwrap();
    let (status, body) = s
        .owner
        .send(
            "PUT",
            "/api/admin/storage/defaults",
            Some(json!({"reserve_gb": 5})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, created) = s.new_qemu_mac("Studio", 64).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let studio = created["instance"]["id"].as_str().unwrap().to_owned();
    s.wait_for(&studio, "stopped").await;

    // Hibernate it: its memory is on disk and QEMU isn't running. A fresh supervisor
    // adopts it as hibernated, as after a server restart.
    let files = s.app.fleet.files(&studio);
    std::fs::create_dir_all(&files.dir).unwrap();
    std::fs::write(files.hibernate(), vec![1u8; 64 * 1024]).unwrap();
    s.app.fleet.forget(&studio).await;
    s.app.fleet.machine(&studio);
    s.wait_for(&studio, "hibernated").await;
    s.app.storage.forget(&studio);
    let (_, mine) = s.owner.send("GET", "/api/storage", None).await;
    let mac = &mine["macs"][0];
    assert_eq!(
        (&mac["instance_id"], &mac["state"]),
        (&json!(studio), &json!("hibernated"))
    );
    assert!(mac["memory_bytes"].as_u64().unwrap() >= 64 * 1024, "{mac}");

    // Only the owner can discard it.
    let (status, _) = s
        .sam
        .send(
            "POST",
            &format!("/api/instances/{studio}/storage/discard-memory"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, view) = s
        .owner
        .send(
            "POST",
            &format!("/api/instances/{studio}/storage/discard-memory"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(
        (&view["id"], &view["state"]),
        (&json!(studio), &json!("stopped"))
    );
    assert!(!files.hibernate().exists(), "the saved memory is gone");
    let (_, mine) = s.owner.send("GET", "/api/storage", None).await;
    assert_eq!(mine["macs"][0]["memory_bytes"], 0, "measured again");

    // Nothing left to discard on a stopped Mac.
    let (status, err) = s
        .owner
        .send(
            "POST",
            &format!("/api/instances/{studio}/storage/discard-memory"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(err["error"]["code"], "conflict");

    // Nor on a running one.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (status, created) = s
        .owner
        .send(
            "POST",
            "/api/instances",
            Some(json!({"name":"Laptop","driver":"remote","remote_host": addr.to_string()})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let laptop = created["instance"]["id"].as_str().unwrap().to_owned();
    let secret = URL_SAFE_NO_PAD
        .decode(
            created["pairing"]
                .as_str()
                .unwrap()
                .split(':')
                .nth(3)
                .unwrap(),
        )
        .unwrap();
    tokio::spawn(fake_guest(listener, laptop.clone(), secret));
    s.wait_for(&laptop, "ready").await;
    let (status, err) = s
        .owner
        .send(
            "POST",
            &format!("/api/instances/{laptop}/storage/discard-memory"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    assert_eq!(err["error"]["code"], "conflict");
    assert_eq!(s.state(&laptop).await, "ready", "left running");

    // A disk over the person's largest allowed is refused before anything is made.
    let app = s.app.clone();
    let count = || async {
        app.db
            .call(|c| c.query_row("SELECT COUNT(*) FROM instances", [], |r| r.get::<_, i64>(0)))
            .await
            .unwrap()
    };
    let before = count().await;
    let (status, _) = s
        .owner
        .send(
            "PUT",
            &format!("/api/admin/users/{owner_id}/storage"),
            Some(json!({"max_disk_gb": 64, "quota_gb": 1})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, err) = s.new_qemu_mac("Big", 128).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["field"], "disk_gb");

    // Over quota: fill Studio's folder past 1 GB. Hard links to one small file are each
    // counted in full, so this costs a few MB of real disk.
    let blob = files.dir.join("blob");
    std::fs::write(&blob, vec![7u8; 4 << 20]).unwrap();
    for i in 0..240 {
        std::fs::hard_link(&blob, files.dir.join(format!("link-{i}"))).unwrap();
    }
    s.app.storage.forget(&studio);
    let (_, mine) = s.owner.send("GET", "/api/storage", None).await;
    assert_eq!(mine["over_quota"], true, "{mine}");
    let (status, err) = s.new_qemu_mac("Another", 64).await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    assert_eq!(err["error"]["code"], "budget");
    assert!(
        err["error"]["message"].as_str().unwrap().contains("1 GB"),
        "{err}"
    );
    // Remote Macs keep their data elsewhere, so the quota doesn't stop them.
    let (status, remote) = s
        .owner
        .send(
            "POST",
            "/api/instances",
            Some(json!({"name":"Mini","driver":"remote","remote_host":"127.0.0.1:1"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{remote}");
    assert_eq!(count().await, before + 1, "only the remote Mac was made");

    // Clearing the quota override lets new Macs through again.
    let (status, _) = s
        .owner
        .send(
            "PUT",
            &format!("/api/admin/users/{owner_id}/storage"),
            Some(json!({"quota_gb": null})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = s.new_qemu_mac("Another", 64).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // And the server's reserve stops them when free space runs low.
    let (status, _) = s
        .owner
        .send(
            "PUT",
            "/api/admin/storage/defaults",
            Some(json!({"reserve_gb": 10_000})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, err) = s.new_qemu_mac("One more", 64).await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    assert_eq!(err["error"]["code"], "budget");
}
