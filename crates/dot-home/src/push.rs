//! Web Push for approvals: a notification reaches the owner's browsers even with no
//! Familiar tab open.
//!
//! Payloads are encrypted for each browser (RFC 8291, `aes128gcm`, via `web-push-native`)
//! and signed with the server's VAPID key (RFC 8292). The key pair is made on first start
//! and kept in `settings`, sealed with the master key like the other secrets.
use crate::{
    crypto::MasterKey,
    db::{Db, now},
    state::App,
    tools::approvals::ApprovalView,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use p256::{
    PublicKey,
    ecdsa::{Signature, SigningKey, signature::Signer},
};
use serde_json::json;
use std::time::Duration;
use web_push_native::{Auth, WebPushBuilder};

/// `settings` key holding the sealed VAPID private key.
const KEY_SETTING: &str = "vapid_private_key";
/// Seal context for that key.
const KEY_CONTEXT: &str = "vapid";
/// How long one push service gets to answer.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
/// Upper bound for all of one approval's pushes.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(30);
/// Browsers one person can register.
pub const MAX_PER_USER: i64 = 20;

/// Push services browsers use today. The server only ever posts to these, so a
/// subscription can't turn it into a proxy for other hosts.
const SERVICES: &[&str] = &[
    // Chrome, Edge on Android, Brave, Opera, Samsung Internet.
    "fcm.googleapis.com",
    // Older Chrome subscriptions.
    "android.googleapis.com",
    // Chromium builds without Google's API keys.
    "jmt17.google.com",
    // Firefox (updates.push.services.mozilla.com).
    "push.services.mozilla.com",
    // Safari, and Home Screen apps on iPhone and iPad.
    "push.apple.com",
    // Edge on Windows (wns2-*.notify.windows.com).
    "notify.windows.com",
];

/// The VAPID key and the client that talks to push services.
pub struct Push {
    key: SigningKey,
    public: String,
    http: reqwest::Client,
    /// `sub` claim: who to contact about these pushes.
    contact: String,
}

impl Push {
    /// Load the VAPID key from the database, creating it on first start.
    pub async fn load_or_create(
        db: &Db,
        master: &MasterKey,
        contact: String,
    ) -> anyhow::Result<Self> {
        let key = match db.setting(KEY_SETTING).await? {
            Some(stored) => {
                let sealed = URL_SAFE_NO_PAD.decode(stored.trim())?;
                let raw = master.open(KEY_CONTEXT, &sealed)?;
                SigningKey::from_slice(&raw)
                    .map_err(|_| anyhow::anyhow!("the stored VAPID key is not a P-256 key"))?
            }
            None => {
                let key = SigningKey::from_slice(&dot_wire::random_bytes::<32>())
                    .or_else(|_| SigningKey::from_slice(&dot_wire::random_bytes::<32>()))
                    .map_err(|_| anyhow::anyhow!("could not make a VAPID key"))?;
                let sealed = master.seal(KEY_CONTEXT, &key.to_bytes());
                db.set_setting(KEY_SETTING, URL_SAFE_NO_PAD.encode(sealed))
                    .await?;
                key
            }
        };
        Self::new(key, contact)
    }

    fn new(key: SigningKey, contact: String) -> anyhow::Result<Self> {
        let public = URL_SAFE_NO_PAD.encode(key.verifying_key().to_encoded_point(false).as_bytes());
        let http = reqwest::Client::builder()
            .timeout(SEND_TIMEOUT)
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("dot-home/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            key,
            public,
            http,
            contact,
        })
    }

    /// Uncompressed P-256 public key, base64url: the `applicationServerKey`.
    pub fn public_key(&self) -> &str {
        &self.public
    }

    /// `Authorization: vapid t=<JWT>, k=<public key>` for `endpoint` (RFC 8292).
    fn authorization(&self, endpoint: &url::Url) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = json!({
            "aud": endpoint.origin().ascii_serialization(),
            // At most 24 hours ahead; push services reject longer.
            "exp": now() + 12 * 3600,
            "sub": self.contact,
        });
        let claims = URL_SAFE_NO_PAD.encode(claims.to_string());
        let signing_input = format!("{header}.{claims}");
        let signature: Signature = self.key.sign(signing_input.as_bytes());
        let jwt = format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        );
        format!("vapid t={jwt}, k={}", self.public)
    }

    /// Encrypt `payload` for one browser and post it.
    async fn send(&self, sub: &Subscription, payload: &[u8], ttl: Duration) -> Outcome {
        let request = match self.request(sub, payload, ttl) {
            Ok(r) => r,
            Err(e) => return Outcome::Failed(e.to_string()),
        };
        match self.http.execute(request).await {
            Ok(r) if r.status().is_success() => Outcome::Delivered,
            Ok(r) if matches!(r.status().as_u16(), 404 | 410) => Outcome::Gone,
            Ok(r) => Outcome::Failed(format!("push service answered {}", r.status())),
            Err(e) if e.is_timeout() => Outcome::Failed("push service timed out".into()),
            Err(e) => Outcome::Failed(format!("push service unreachable: {e}")),
        }
    }

    fn request(
        &self,
        sub: &Subscription,
        payload: &[u8],
        ttl: Duration,
    ) -> anyhow::Result<reqwest::Request> {
        let endpoint = url::Url::parse(&sub.endpoint)?;
        let (public, auth) = browser_keys(&sub.p256dh, &sub.auth)?;
        let mut request = WebPushBuilder::new(endpoint.as_str().parse()?, public, auth)
            .with_valid_duration(ttl)
            .build(payload.to_vec())
            .map_err(|e| anyhow::anyhow!("encrypting the push: {e}"))?;
        let headers = request.headers_mut();
        headers.insert(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_str(&self.authorization(&endpoint))?,
        );
        // Someone is waiting on the answer: wake the device.
        headers.insert("urgency", http::HeaderValue::from_static("high"));
        Ok(reqwest::Request::try_from(request)?)
    }
}

/// What a push service said.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Delivered,
    /// 404 or 410: the browser unsubscribed; forget it.
    Gone,
    Failed(String),
}

/// A stored subscription.
#[derive(Debug, Clone)]
pub struct Subscription {
    /// Push service URL.
    pub endpoint: String,
    /// Browser public key, base64url.
    pub p256dh: String,
    /// Authentication secret, base64url.
    pub auth: String,
}

fn b64(text: &str) -> anyhow::Result<Vec<u8>> {
    Ok(URL_SAFE_NO_PAD.decode(text.trim().trim_end_matches('='))?)
}

/// Decode and check a browser's `p256dh` and `auth`.
fn browser_keys(p256dh: &str, auth: &str) -> anyhow::Result<(PublicKey, Auth)> {
    let public = PublicKey::from_sec1_bytes(&b64(p256dh)?)
        .map_err(|_| anyhow::anyhow!("p256dh is not a P-256 public key"))?;
    let auth = b64(auth)?;
    anyhow::ensure!(auth.len() == 16, "auth must be 16 bytes");
    Ok((public, Auth::clone_from_slice(&auth)))
}

/// Check a subscription from a browser; the error is a sentence for the person.
pub fn validate(
    endpoint: &str,
    p256dh: &str,
    auth: &str,
) -> Result<(), (&'static str, &'static str)> {
    let bad_endpoint = ("endpoint", "This browser's push service isn't supported.");
    if endpoint.len() > 2048 {
        return Err(bad_endpoint);
    }
    let url = url::Url::parse(endpoint).map_err(|_| bad_endpoint)?;
    let host = match url.host() {
        Some(url::Host::Domain(h)) => h.to_ascii_lowercase(),
        _ => return Err(bad_endpoint),
    };
    let known = SERVICES
        .iter()
        .any(|s| host == *s || host.ends_with(&format!(".{s}")));
    if url.scheme() != "https"
        || !known
        || url.port().is_some_and(|p| p != 443)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(bad_endpoint);
    }
    if PublicKey::from_sec1_bytes(&b64(p256dh).unwrap_or_default()).is_err() {
        return Err(("keys.p256dh", "The browser's push key isn't valid."));
    }
    if b64(auth).ok().map(|a| a.len()) != Some(16) {
        return Err(("keys.auth", "The browser's push secret isn't valid."));
    }
    Ok(())
}

/// Push a new approval to its owner's browsers, in the background. Never blocks.
pub fn approval_created(app: &App, view: &ApprovalView) {
    let (app, view) = (app.clone(), view.clone());
    tokio::spawn(async move {
        match tokio::time::timeout(NOTIFY_TIMEOUT, notify(&app, &view)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(approval = %view.id, error = %e, "approval push"),
            Err(_) => tracing::warn!(approval = %view.id, "approval push timed out"),
        }
    });
}

/// The notification body for an approval.
pub fn payload(view: &ApprovalView) -> serde_json::Value {
    let mut chars = view.title.chars();
    let action = chars
        .next()
        .map(|c| c.to_lowercase().chain(chars).collect::<String>())
        .unwrap_or_default();
    let who = view.client.as_deref().unwrap_or("your assistant");
    json!({
        "title": format!("Allow {who} to {action}?"),
        "body": view.summary,
        "url": format!("/macs/{}", view.instance_id),
        "approval_id": view.id,
        "instance_id": view.instance_id,
    })
}

async fn notify(app: &App, view: &ApprovalView) -> anyhow::Result<()> {
    let (iid, aid, t) = (view.instance_id.clone(), view.id.clone(), now());
    // Owner, their preference and the one-push-per-approval claim, in one go.
    let target = app
        .db
        .call(move |c| {
            use rusqlite::OptionalExtension;
            let owner: Option<(String, Option<String>)> = c
                .query_row(
                    "SELECT u.id, u.preferences FROM instances i JOIN users u ON u.id = i.user_id
                     WHERE i.id = ?1 AND u.disabled = 0",
                    [&iid],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((uid, prefs)) = owner else {
                return Ok(None);
            };
            if !crate::api::preferences::approval_notifications(prefs.as_deref()) {
                return Ok(None);
            }
            let claimed = c.execute(
                "UPDATE approvals SET pushed_at = ?2 WHERE id = ?1 AND pushed_at IS NULL",
                rusqlite::params![aid, t],
            )?;
            if claimed == 0 {
                return Ok(None);
            }
            let mut s = c.prepare(
                "SELECT endpoint, p256dh, auth FROM push_subscriptions WHERE user_id = ?1",
            )?;
            let subs = s
                .query_map([&uid], |r| {
                    Ok(Subscription {
                        endpoint: r.get(0)?,
                        p256dh: r.get(1)?,
                        auth: r.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Some(subs))
        })
        .await?;
    let Some(subs) = target else { return Ok(()) };
    let body = payload(view).to_string();
    // Pointless once the request can't be answered any more.
    let ttl = Duration::from_secs(u64::try_from(crate::tools::approvals::EXPIRES_S).unwrap_or(900));
    let outcomes =
        futures_util::future::join_all(subs.iter().map(|s| app.push.send(s, body.as_bytes(), ttl)))
            .await;
    for (sub, outcome) in subs.into_iter().zip(outcomes) {
        let host = url::Url::parse(&sub.endpoint)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default();
        match outcome {
            Outcome::Delivered => {}
            Outcome::Gone => {
                tracing::info!(%host, "push subscription expired; removing it");
                let endpoint = sub.endpoint;
                app.db
                    .call(move |c| {
                        c.execute(
                            "DELETE FROM push_subscriptions WHERE endpoint = ?1",
                            [endpoint],
                        )
                    })
                    .await?;
            }
            // The endpoint is a bearer URL; log only its host.
            Outcome::Failed(why) => tracing::warn!(%host, error = %why, "approval push failed"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use p256::ecdsa::{VerifyingKey, signature::Verifier};
    use p256::elliptic_curve::sec1::ToEncodedPoint;

    fn push() -> Push {
        Push::new(
            SigningKey::from_slice(&[7u8; 32]).unwrap(),
            "https://home.example.com".into(),
        )
        .unwrap()
    }

    #[test]
    fn only_known_push_services_and_valid_keys_are_accepted() {
        let browser = p256::SecretKey::from_slice(&[9u8; 32]).unwrap();
        let p256dh =
            URL_SAFE_NO_PAD.encode(browser.public_key().to_encoded_point(false).as_bytes());
        let auth = URL_SAFE_NO_PAD.encode([1u8; 16]);
        for ok in [
            "https://fcm.googleapis.com/fcm/send/abc",
            "https://updates.push.services.mozilla.com/wpush/v2/abc",
            "https://web.push.apple.com/QGx",
            "https://jmt17.google.com/fcm/send/abc",
            "https://wns2-par02p.notify.windows.com/w/?token=abc",
        ] {
            assert_eq!(validate(ok, &p256dh, &auth), Ok(()), "{ok}");
        }
        for bad in [
            "http://fcm.googleapis.com/fcm/send/abc",
            "https://127.0.0.1/x",
            "https://localhost/x",
            "https://evil.example/x",
            "https://fcm.googleapis.com.evil.example/x",
            "https://notfcm.googleapis.com.evil/x",
            "https://fcm.googleapis.com:8443/x",
            "https://user@fcm.googleapis.com/x",
            "not a url",
        ] {
            assert_eq!(
                validate(bad, &p256dh, &auth).unwrap_err().0,
                "endpoint",
                "{bad}"
            );
        }
        let ok = "https://fcm.googleapis.com/fcm/send/abc";
        assert_eq!(validate(ok, "AAAA", &auth).unwrap_err().0, "keys.p256dh");
        assert_eq!(
            validate(ok, &p256dh, &URL_SAFE_NO_PAD.encode([1u8; 8]))
                .unwrap_err()
                .0,
            "keys.auth"
        );
    }

    #[test]
    fn vapid_header_is_a_valid_es256_jwt_for_the_push_origin() {
        let p = push();
        let endpoint = url::Url::parse("https://fcm.googleapis.com/fcm/send/abc").unwrap();
        let header = p.authorization(&endpoint);
        let rest = header.strip_prefix("vapid t=").unwrap();
        let (jwt, k) = rest.split_once(", k=").unwrap();
        assert_eq!(k, p.public_key());
        let key_bytes = URL_SAFE_NO_PAD.decode(k).unwrap();
        assert_eq!(key_bytes.len(), 65);
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let claims: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://fcm.googleapis.com");
        assert_eq!(claims["sub"], "https://home.example.com");
        let exp = claims["exp"].as_i64().unwrap();
        assert!(exp > now() && exp <= now() + 24 * 3600);
        let sig = Signature::from_slice(&URL_SAFE_NO_PAD.decode(parts[2]).unwrap()).unwrap();
        let verifying = VerifyingKey::from_sec1_bytes(&key_bytes).unwrap();
        verifying
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
            .unwrap();
    }

    #[test]
    fn the_request_is_encrypted_for_the_browser_and_only_it() {
        let p = push();
        let browser = p256::SecretKey::from_slice(&[9u8; 32]).unwrap();
        let auth = [3u8; 16];
        let sub = Subscription {
            endpoint: "https://fcm.googleapis.com/fcm/send/abc".into(),
            p256dh: URL_SAFE_NO_PAD.encode(browser.public_key().to_encoded_point(false).as_bytes()),
            auth: URL_SAFE_NO_PAD.encode(auth),
        };
        let r = p
            .request(&sub, br#"{"title":"hi"}"#, Duration::from_secs(900))
            .unwrap();
        assert_eq!(r.method(), "POST");
        assert_eq!(r.url().as_str(), sub.endpoint);
        let h = r.headers();
        assert_eq!(h["content-encoding"], "aes128gcm");
        assert_eq!(h["ttl"], "900");
        assert_eq!(h["urgency"], "high");
        assert!(h["authorization"].to_str().unwrap().starts_with("vapid t="));
        let body = r.body().unwrap().as_bytes().unwrap().to_vec();
        let plain =
            web_push_native::decrypt(body.clone(), &browser, &Auth::clone_from_slice(&auth))
                .unwrap();
        assert_eq!(plain, br#"{"title":"hi"}"#);
        let other = p256::SecretKey::from_slice(&[8u8; 32]).unwrap();
        assert!(web_push_native::decrypt(body, &other, &Auth::clone_from_slice(&auth)).is_err());
    }

    #[tokio::test]
    async fn the_key_is_made_once_and_kept_sealed() {
        let db = Db::memory().unwrap();
        let master = MasterKey::from_bytes(&[5u8; 32]).unwrap();
        let a = Push::load_or_create(&db, &master, "x".into())
            .await
            .unwrap();
        let b = Push::load_or_create(&db, &master, "x".into())
            .await
            .unwrap();
        assert_eq!(a.public_key(), b.public_key());
        let stored = db.setting(KEY_SETTING).await.unwrap().unwrap();
        assert!(!stored.contains(&URL_SAFE_NO_PAD.encode(a.key.to_bytes())));
        let wrong = MasterKey::from_bytes(&[6u8; 32]).unwrap();
        assert!(Push::load_or_create(&db, &wrong, "x".into()).await.is_err());
    }

    #[test]
    fn payload_reads_like_the_portal() {
        let view = ApprovalView {
            id: "a1".into(),
            instance_id: "i1".into(),
            tool: "send_email".into(),
            title: "Send email".into(),
            summary: "To sam@example.com: Lunch?".into(),
            client: Some("Claude".into()),
            created_at: String::new(),
            expires_at: String::new(),
            status: "pending".into(),
            arguments: json!({}),
        };
        assert_eq!(
            payload(&view),
            json!({"title":"Allow Claude to send email?","body":"To sam@example.com: Lunch?",
                   "url":"/macs/i1","approval_id":"a1","instance_id":"i1"})
        );
    }
}
