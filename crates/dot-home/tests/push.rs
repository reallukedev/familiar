//! Web Push subscriptions: the VAPID key, adding and removing a browser, and who may.
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
// A valid P-256 point (RFC 8292's example key) and a 16-byte secret.
const P256DH: &str =
    "BA1Hxzyi1RUM1b5wjxsn7nGxAszw2u61m164i3MrAIxHF6YK5h4SDYic-dRuU_RCPCfA5aq9ojSwk5Y2EmClBPs";
const AUTH: &str = "AAECAwQFBgcICQoLDA0ODw";

struct Client {
    router: Router,
    cookie: Option<String>,
    csrf: Option<String>,
}

impl Client {
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
}

#[tokio::test]
async fn browsers_subscribe_and_unsubscribe() {
    let dir = tempfile::tempdir().unwrap();
    let mut config: dot_home::config::Config =
        toml::from_str(&format!("public_url = '{ORIGIN}'")).unwrap();
    config.data_dir = dir.path().to_path_buf();
    let app = dot_home::build(config).await.unwrap();
    let mut c = Client {
        router: dot_home::router(app.clone()),
        cookie: None,
        csrf: None,
    };

    // Signed out: nothing.
    let (status, _) = c.send("GET", "/api/push/key", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let url = std::fs::read_to_string(dir.path().join("setup-url.txt")).unwrap();
    let token = url.split("token=").nth(1).unwrap().to_owned();
    let (status, _) = c.send("POST", "/api/setup", Some(json!({"token":token,"username":"alex","display_name":"Alex","password":"a long passphrase"}))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, begin) = c.send("POST", "/api/account/totp", None).await;
    let secret = data_encoding::BASE32_NOPAD
        .decode(begin["secret"].as_str().unwrap().as_bytes())
        .unwrap();
    let code = dot_home::auth::totp::code(&secret, dot_home::db::now());
    c.send(
        "POST",
        "/api/account/totp/confirm",
        Some(json!({"code": code})),
    )
    .await;

    // The key is an uncompressed P-256 point and stays the same across restarts.
    let (status, key) = c.send("GET", "/api/push/key", None).await;
    assert_eq!(status, StatusCode::OK);
    let public = key["public_key"].as_str().unwrap().to_owned();
    let raw = data_encoding::BASE64URL_NOPAD
        .decode(public.as_bytes())
        .unwrap();
    assert_eq!((raw.len(), raw[0]), (65, 4));
    let again = dot_home::push::Push::load_or_create(&app.db, &app.key, ORIGIN.into())
        .await
        .unwrap();
    assert_eq!(again.public_key(), public);

    // Only real push services, with valid keys.
    let (status, bad) = c
        .send(
            "POST",
            "/api/push/subscriptions",
            Some(json!({"endpoint":"https://127.0.0.1/x","keys":{"p256dh":P256DH,"auth":AUTH}})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    assert_eq!(bad["error"]["field"], "endpoint");
    let endpoint = "https://fcm.googleapis.com/fcm/send/abc123";
    let (status, bad) = c
        .send(
            "POST",
            "/api/push/subscriptions",
            Some(json!({"endpoint":endpoint,"keys":{"p256dh":"AAAA","auth":AUTH}})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");

    // Subscribing twice keeps one row.
    for _ in 0..2 {
        let (status, body) = c
            .send(
                "POST",
                "/api/push/subscriptions",
                Some(json!({"endpoint":endpoint,"keys":{"p256dh":P256DH,"auth":AUTH}})),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let count = || async {
        app.db
            .call(|c| {
                c.query_row("SELECT COUNT(*) FROM push_subscriptions", [], |r| {
                    r.get::<_, i64>(0)
                })
            })
            .await
            .unwrap()
    };
    assert_eq!(count().await, 1);

    // A change without the CSRF header is refused.
    let csrf = c.csrf.take();
    let (_, refused) = c
        .send(
            "DELETE",
            "/api/push/subscriptions",
            Some(json!({"endpoint":endpoint})),
        )
        .await;
    assert_eq!(refused["error"]["code"], "csrf");
    c.csrf = csrf;
    assert_eq!(count().await, 1);

    let (status, _) = c
        .send(
            "DELETE",
            "/api/push/subscriptions",
            Some(json!({"endpoint":endpoint})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(count().await, 0);

    // Preferences are reachable, and switching notifications on is saved.
    let (status, prefs) = c
        .send(
            "PUT",
            "/api/account/preferences",
            Some(json!({"approval_notifications": true})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{prefs}");
    assert_eq!(prefs["approval_notifications"], true);
}
