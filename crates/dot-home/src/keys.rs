//! Per-person encryption at rest (docs/home/SECURITY.md "Encryption at rest").
//!
//! Each person has a random 256-bit data key. The database only ever holds it wrapped:
//! once with a key derived from their password (Argon2id) and once with their recovery
//! key. Signing in unwraps it into the [`Keyring`], where it stays until the server
//! restarts; each Mac's disk key is derived from it with HKDF and the Mac's id.
use crate::db::{Db, DbError, now};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use data_encoding::BASE32_NOPAD;
use hkdf::Hkdf;
use rusqlite::OptionalExtension;
use sha2::Sha256;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

/// Recovery keys carry 160 bits: 32 base32 characters, shown as 8 groups of 4.
const RECOVERY_BYTES: usize = 20;

/// Argon2id settings for new password wraps. Stored with each wrap, so they can change.
pub const KDF: KdfParams = KdfParams {
    memory_kib: 64 * 1024,
    passes: 3,
    lanes: 1,
};

/// Argon2id settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    /// Memory in KiB.
    pub memory_kib: u32,
    /// Iterations.
    pub passes: u32,
    /// Parallelism.
    pub lanes: u32,
}

/// A person's data key. Cheap to clone; the bytes are wiped when the last copy drops.
#[derive(Clone)]
pub struct DataKey(Arc<Zeroizing<[u8; 32]>>);

impl std::fmt::Debug for DataKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DataKey(..)")
    }
}

impl DataKey {
    /// A new random key.
    pub fn random() -> Self {
        Self(Arc::new(Zeroizing::new(dot_wire::random_bytes::<32>())))
    }

    /// The key for one Mac's disks and saved memory.
    pub fn mac_key(&self, instance_id: &str) -> MacKey {
        MacKey(derive(
            &self.0[..],
            None,
            format!("familiar mac v1:{instance_id}").as_bytes(),
        ))
    }

    /// Same key material (tests).
    #[cfg(test)]
    pub fn same_as(&self, other: &Self) -> bool {
        self.0[..] == other.0[..]
    }
}

/// One Mac's key, derived from its owner's [`DataKey`].
#[derive(Clone)]
pub struct MacKey(Zeroizing<[u8; 32]>);

impl std::fmt::Debug for MacKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MacKey(..)")
    }
}

impl MacKey {
    /// From raw bytes (tests).
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// The passphrase QEMU's LUKS layer opens the disks with. The key is random, so the
    /// passphrase needs no stretching; QEMU's own PBKDF runs at its minimum.
    pub fn passphrase(&self) -> Zeroizing<String> {
        Zeroizing::new(STANDARD.encode(&self.0[..]))
    }

    /// A one-off key for one saved-memory file, from that file's random salt.
    pub fn memory_key(&self, salt: &[u8]) -> Zeroizing<[u8; 32]> {
        derive(&self.0[..], Some(salt), b"familiar memory v1")
    }
}

fn derive(ikm: &[u8], salt: Option<&[u8]>, info: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut out = Zeroizing::new([0u8; 32]);
    // 32 bytes is far below HKDF-SHA256's 8160-byte limit, so expand can't fail.
    let _ = Hkdf::<Sha256>::new(salt, ikm).expand(info, &mut out[..]);
    out
}

/// A recovery key: the second way to unwrap the data key.
pub struct RecoveryKey(Zeroizing<[u8; RECOVERY_BYTES]>);

impl RecoveryKey {
    /// A new random recovery key.
    pub fn random() -> Self {
        Self(Zeroizing::new(dot_wire::random_bytes::<RECOVERY_BYTES>()))
    }

    /// `ABCD-EFGH-…`: 8 groups of 4 base32 characters.
    pub fn display(&self) -> String {
        let text = Zeroizing::new(BASE32_NOPAD.encode(&self.0[..]));
        text.as_bytes()
            .chunks(4)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect::<Vec<_>>()
            .join("-")
    }

    /// Read a typed key: case, spaces and dashes don't matter, and the digits people
    /// mistake for letters (0, 1, 8) are read as O, I and B.
    pub fn parse(text: &str) -> Option<Self> {
        let clean: Zeroizing<String> = Zeroizing::new(
            text.chars()
                .filter(|c| !c.is_whitespace() && *c != '-')
                .map(|c| match c.to_ascii_uppercase() {
                    '0' => 'O',
                    '1' => 'I',
                    '8' => 'B',
                    c => c,
                })
                .collect(),
        );
        let bytes = Zeroizing::new(BASE32_NOPAD.decode(clean.as_bytes()).ok()?);
        let raw: [u8; RECOVERY_BYTES] = bytes.as_slice().try_into().ok()?;
        Some(Self(Zeroizing::new(raw)))
    }

    fn kek(&self, user_id: &str) -> Zeroizing<[u8; 32]> {
        // 160 random bits need no stretching.
        derive(
            &self.0[..],
            Some(user_id.as_bytes()),
            b"familiar recovery v1",
        )
    }
}

/// Wrapped key material, as stored in `user_keys`.
#[derive(Clone)]
pub struct Stored {
    salt: Vec<u8>,
    params: KdfParams,
    password_wrap: Vec<u8>,
    recovery_wrap: Vec<u8>,
    /// The person confirmed they saved their recovery key.
    pub recovery_saved: bool,
}

fn password_kek(
    password: &str,
    salt: &[u8],
    params: KdfParams,
) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let params = argon2::Params::new(params.memory_kib, params.passes, params.lanes, Some(32))
        .map_err(|e| anyhow::anyhow!("key derivation settings: {e}"))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(password.as_bytes(), salt, &mut out[..])
        .map_err(|e| anyhow::anyhow!("key derivation: {e}"))?;
    Ok(out)
}

fn aad(user_id: &str, purpose: &str) -> String {
    format!("familiar data key v1:{purpose}:{user_id}")
}

fn wrap(kek: &[u8; 32], user_id: &str, purpose: &str, key: &DataKey) -> Vec<u8> {
    let cipher = Aes256Gcm::new(kek.into());
    let nonce = dot_wire::random_bytes::<12>();
    let mut out = nonce.to_vec();
    // Encryption with a valid key and a fresh nonce can't fail.
    if let Ok(ct) = cipher.encrypt(
        Nonce::from_slice(&nonce),
        Payload {
            msg: &key.0[..],
            aad: aad(user_id, purpose).as_bytes(),
        },
    ) {
        out.extend(ct);
    }
    out
}

fn unwrap(kek: &[u8; 32], user_id: &str, purpose: &str, blob: &[u8]) -> Option<DataKey> {
    if blob.len() <= 12 {
        return None;
    }
    let (nonce, ct) = blob.split_at(12);
    let plain = Zeroizing::new(
        Aes256Gcm::new(kek.into())
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ct,
                    aad: aad(user_id, purpose).as_bytes(),
                },
            )
            .ok()?,
    );
    let bytes: [u8; 32] = plain.as_slice().try_into().ok()?;
    Some(DataKey(Arc::new(Zeroizing::new(bytes))))
}

impl Stored {
    /// Wrap `key` with a password and a recovery key. Slow (Argon2id): call off the
    /// async threads.
    pub fn seal(
        user_id: &str,
        password: &str,
        key: &DataKey,
        recovery: &RecoveryKey,
    ) -> anyhow::Result<Self> {
        let salt = dot_wire::random_bytes::<16>().to_vec();
        let kek = password_kek(password, &salt, KDF)?;
        Ok(Self {
            password_wrap: wrap(&kek, user_id, "password", key),
            recovery_wrap: wrap(&recovery.kek(user_id), user_id, "recovery", key),
            salt,
            params: KDF,
            recovery_saved: false,
        })
    }

    /// Unwrap with the password. `None` when it's the wrong password. Slow.
    pub fn open_with_password(&self, user_id: &str, password: &str) -> Option<DataKey> {
        let kek = password_kek(password, &self.salt, self.params).ok()?;
        unwrap(&kek, user_id, "password", &self.password_wrap)
    }

    /// Unwrap with the recovery key. `None` when it's the wrong key.
    pub fn open_with_recovery(&self, user_id: &str, recovery: &RecoveryKey) -> Option<DataKey> {
        unwrap(
            &recovery.kek(user_id),
            user_id,
            "recovery",
            &self.recovery_wrap,
        )
    }

    /// The same key wrapped with a new password (fresh salt, current settings). Slow.
    pub fn with_password(
        &self,
        user_id: &str,
        password: &str,
        key: &DataKey,
    ) -> anyhow::Result<Self> {
        let salt = dot_wire::random_bytes::<16>().to_vec();
        let kek = password_kek(password, &salt, KDF)?;
        Ok(Self {
            password_wrap: wrap(&kek, user_id, "password", key),
            salt,
            params: KDF,
            ..self.clone()
        })
    }

    /// The same key wrapped with a new recovery key; the old one stops working.
    pub fn with_recovery(&self, user_id: &str, key: &DataKey, recovery: &RecoveryKey) -> Self {
        Self {
            recovery_wrap: wrap(&recovery.kek(user_id), user_id, "recovery", key),
            recovery_saved: false,
            ..self.clone()
        }
    }

    /// Insert or replace the row for `user_id` (inside the caller's transaction).
    pub fn save(&self, c: &rusqlite::Connection, user_id: &str) -> rusqlite::Result<()> {
        let t = now();
        c.execute(
            "INSERT INTO user_keys (user_id, kdf_salt, kdf_memory_kib, kdf_passes, kdf_lanes, password_wrap, recovery_wrap, recovery_saved, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
             ON CONFLICT(user_id) DO UPDATE SET kdf_salt = excluded.kdf_salt, kdf_memory_kib = excluded.kdf_memory_kib,
               kdf_passes = excluded.kdf_passes, kdf_lanes = excluded.kdf_lanes, password_wrap = excluded.password_wrap,
               recovery_wrap = excluded.recovery_wrap, recovery_saved = excluded.recovery_saved, updated_at = excluded.updated_at",
            rusqlite::params![
                user_id,
                self.salt,
                self.params.memory_kib,
                self.params.passes,
                self.params.lanes,
                self.password_wrap,
                self.recovery_wrap,
                self.recovery_saved,
                t
            ],
        )
        .map(|_| ())
    }

    /// Read the row for `user_id`.
    pub fn read(c: &rusqlite::Connection, user_id: &str) -> rusqlite::Result<Option<Self>> {
        c.query_row(
            "SELECT kdf_salt, kdf_memory_kib, kdf_passes, kdf_lanes, password_wrap, recovery_wrap, recovery_saved
             FROM user_keys WHERE user_id = ?1",
            [user_id],
            |r| {
                Ok(Self {
                    salt: r.get(0)?,
                    params: KdfParams {
                        memory_kib: r.get(1)?,
                        passes: r.get(2)?,
                        lanes: r.get(3)?,
                    },
                    password_wrap: r.get(4)?,
                    recovery_wrap: r.get(5)?,
                    recovery_saved: r.get(6)?,
                })
            },
        )
        .optional()
    }
}

/// Load a person's wrapped keys.
pub async fn load(db: &Db, user_id: &str) -> Result<Option<Stored>, DbError> {
    let uid = user_id.to_owned();
    db.call(move |c| Stored::read(c, &uid)).await
}

/// What a password (or recovery key) opened.
#[derive(Debug, Clone)]
pub struct Opened {
    /// The data key.
    pub key: DataKey,
    /// A recovery key made just now, to show the person once.
    pub recovery_key: Option<String>,
}

/// Unwrap a person's data key with their password, creating their keys on first use
/// (a new account, or one from before encryption). `None` when the password doesn't
/// open the stored wrap.
pub async fn open_or_create(
    db: &Db,
    user_id: &str,
    password: &str,
) -> anyhow::Result<Option<Opened>> {
    let password = Zeroizing::new(password.to_owned());
    // Two first sign-ins at once: the second finds the first one's keys and opens them.
    for _ in 0..2 {
        if let Some(stored) = load(db, user_id).await? {
            let (uid, pw) = (user_id.to_owned(), password.clone());
            let key =
                tokio::task::spawn_blocking(move || stored.open_with_password(&uid, &pw)).await?;
            return Ok(key.map(|key| Opened {
                key,
                recovery_key: None,
            }));
        }
        let key = DataKey::random();
        let recovery = RecoveryKey::random();
        let shown = recovery.display();
        let (uid, pw, k2) = (user_id.to_owned(), password.clone(), key.clone());
        let stored =
            tokio::task::spawn_blocking(move || Stored::seal(&uid, &pw, &k2, &recovery)).await??;
        let uid = user_id.to_owned();
        let created = db
            .call(move |c| {
                let tx = c.transaction()?;
                if Stored::read(&tx, &uid)?.is_some() {
                    return Ok(false);
                }
                stored.save(&tx, &uid)?;
                tx.commit()?;
                Ok(true)
            })
            .await?;
        if created {
            return Ok(Some(Opened {
                key,
                recovery_key: Some(shown),
            }));
        }
    }
    Ok(None)
}

/// Replace the recovery wrap with a new random key; returns it for showing once.
pub async fn new_recovery_key(db: &Db, user_id: &str, key: &DataKey) -> anyhow::Result<String> {
    let stored = load(db, user_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no keys for this account"))?;
    let recovery = RecoveryKey::random();
    let shown = recovery.display();
    let next = stored.with_recovery(user_id, key, &recovery);
    let uid = user_id.to_owned();
    db.call(move |c| next.save(c, &uid)).await?;
    Ok(shown)
}

/// The person saved their recovery key.
pub async fn mark_saved(db: &Db, user_id: &str) -> Result<(), DbError> {
    let uid = user_id.to_owned();
    db.call(move |c| {
        c.execute(
            "UPDATE user_keys SET recovery_saved = 1 WHERE user_id = ?1",
            [uid],
        )
        .map(|_| ())
    })
    .await
}

/// Whether the person still has to save a recovery key (or has none yet).
pub async fn recovery_key_needed(db: &Db, user_id: &str) -> bool {
    !matches!(load(db, user_id).await, Ok(Some(s)) if s.recovery_saved)
}

/// How long an unwrapped key waits for the two-factor step of a sign-in.
const PENDING: Duration = Duration::from_secs(300);

struct Pending {
    user_id: String,
    opened: Opened,
    expires: Instant,
}

/// Unwrapped data keys, by user id. Lives only in memory: a restart locks everyone.
#[derive(Default)]
pub struct Keyring {
    unlocked: Mutex<HashMap<String, DataKey>>,
    /// Keys a password opened while the sign-in waits for its authenticator code, by
    /// challenge hash. The key only counts as unlocked once the code is right.
    pending: Mutex<HashMap<Vec<u8>, Pending>>,
}

impl Keyring {
    fn unlocked(&self) -> std::sync::MutexGuard<'_, HashMap<String, DataKey>> {
        self.unlocked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, HashMap<Vec<u8>, Pending>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Keep a person's key in memory.
    pub fn insert(&self, user_id: &str, key: DataKey) {
        self.unlocked().insert(user_id.to_owned(), key);
    }

    /// The person's key, when unlocked.
    pub fn get(&self, user_id: &str) -> Option<DataKey> {
        self.unlocked().get(user_id).cloned()
    }

    /// Whether the person's key is in memory.
    pub fn is_unlocked(&self, user_id: &str) -> bool {
        self.unlocked().contains_key(user_id)
    }

    /// Forget a person's key (their password was reset, or the account deleted).
    pub fn remove(&self, user_id: &str) {
        self.unlocked().remove(user_id);
        self.pending().retain(|_, p| p.user_id != user_id);
    }

    /// Forget every key, as a restart does.
    pub fn clear(&self) {
        self.unlocked().clear();
        self.pending().clear();
    }

    /// Hold a key until the sign-in identified by `challenge_hash` finishes.
    pub fn hold(&self, challenge_hash: Vec<u8>, user_id: &str, opened: Opened) {
        let t = Instant::now();
        let mut pending = self.pending();
        pending.retain(|_, p| p.expires > t);
        pending.insert(
            challenge_hash,
            Pending {
                user_id: user_id.to_owned(),
                opened,
                expires: t + PENDING,
            },
        );
    }

    /// The key held for a finished sign-in, if it's still there.
    pub fn take(&self, challenge_hash: &[u8], user_id: &str) -> Option<Opened> {
        let p = self.pending().remove(challenge_hash)?;
        (p.user_id == user_id && p.expires > Instant::now()).then_some(p.opened)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fast() -> KdfParams {
        KdfParams {
            memory_kib: 1024,
            passes: 1,
            lanes: 1,
        }
    }

    fn sealed(uid: &str, password: &str, key: &DataKey, recovery: &RecoveryKey) -> Stored {
        let mut s = Stored::seal(uid, password, key, recovery).unwrap();
        // Re-wrap with cheap settings so the rest of the test is quick; the stored
        // parameters are what unwrapping uses.
        let salt = s.salt.clone();
        let kek = password_kek(password, &salt, fast()).unwrap();
        s.params = fast();
        s.password_wrap = wrap(&kek, uid, "password", key);
        s
    }

    #[test]
    fn wraps_open_with_the_password_or_the_recovery_key_only() {
        let key = DataKey::random();
        let recovery = RecoveryKey::random();
        let s = sealed("u1", "correct horse battery", &key, &recovery);
        assert!(
            s.open_with_password("u1", "correct horse battery")
                .unwrap()
                .same_as(&key)
        );
        assert!(
            s.open_with_password("u1", "wrong horse battery").is_none(),
            "wrong password"
        );
        assert!(
            s.open_with_password("u2", "correct horse battery")
                .is_none(),
            "a wrap is bound to its account"
        );
        assert!(s.open_with_recovery("u1", &recovery).unwrap().same_as(&key));
        assert!(s.open_with_recovery("u1", &RecoveryKey::random()).is_none());
        // Nothing stored is the key itself.
        for blob in [&s.password_wrap, &s.recovery_wrap, &s.salt] {
            assert!(!blob.windows(32).any(|w| w == &key.0[..]));
        }
    }

    #[test]
    fn new_password_and_new_recovery_key_rewrap_the_same_key() {
        let key = DataKey::random();
        let recovery = RecoveryKey::random();
        let s = sealed("u1", "first password!", &key, &recovery);
        let mut changed = s.with_password("u1", "second password!", &key).unwrap();
        changed.params = KDF;
        assert!(
            changed
                .open_with_password("u1", "second password!")
                .unwrap()
                .same_as(&key)
        );
        assert!(
            changed
                .open_with_password("u1", "first password!")
                .is_none()
        );
        assert!(
            changed.open_with_recovery("u1", &recovery).is_some(),
            "a password change keeps the recovery key"
        );
        let fresh = RecoveryKey::random();
        let mut saved = changed.clone();
        saved.recovery_saved = true;
        let replaced = saved.with_recovery("u1", &key, &fresh);
        assert!(!replaced.recovery_saved, "a new key must be saved again");
        assert!(replaced.open_with_recovery("u1", &recovery).is_none());
        assert!(
            replaced
                .open_with_recovery("u1", &fresh)
                .unwrap()
                .same_as(&key)
        );
    }

    #[test]
    fn recovery_keys_display_grouped_and_parse_forgivingly() {
        let r = RecoveryKey::random();
        let shown = r.display();
        assert_eq!(shown.len(), 39);
        assert_eq!(shown.split('-').count(), 8);
        assert!(shown.split('-').all(|g| g.len() == 4));
        let back = RecoveryKey::parse(&shown.to_lowercase().replace('-', " ")).unwrap();
        assert_eq!(back.0[..], r.0[..]);
        // O typed as 0 still reads.
        let zeroed = shown.replace('O', "0");
        assert_eq!(RecoveryKey::parse(&zeroed).unwrap().0[..], r.0[..]);
        assert!(RecoveryKey::parse("ABCD-EFGH").is_none());
        assert!(RecoveryKey::parse("not a key at all!").is_none());
    }

    #[test]
    fn mac_keys_differ_per_mac_and_never_print() {
        let key = DataKey::random();
        let a = key.mac_key("m1");
        assert_eq!(a.passphrase(), key.mac_key("m1").passphrase());
        assert_ne!(a.passphrase(), key.mac_key("m2").passphrase());
        assert_ne!(a.passphrase(), DataKey::random().mac_key("m1").passphrase());
        assert_ne!(a.memory_key(b"salt-a")[..], a.memory_key(b"salt-b")[..]);
        assert_eq!(format!("{a:?} {key:?}"), "MacKey(..) DataKey(..)");
    }

    #[test]
    fn keyring_holds_keys_until_cleared_and_pending_needs_the_right_user() {
        let ring = Keyring::default();
        let key = DataKey::random();
        ring.insert("u1", key.clone());
        assert!(ring.is_unlocked("u1"));
        assert!(!ring.is_unlocked("u2"));
        ring.hold(
            b"challenge".to_vec(),
            "u2",
            Opened {
                key: key.clone(),
                recovery_key: None,
            },
        );
        assert!(ring.take(b"challenge", "u1").is_none(), "wrong user");
        assert!(ring.take(b"challenge", "u2").is_none(), "taken once");
        ring.clear();
        assert!(!ring.is_unlocked("u1"), "a restart locks everyone");
    }

    #[tokio::test]
    async fn first_unlock_creates_keys_and_later_ones_open_them() {
        let db = Db::memory().unwrap();
        db.call(|c| {
            c.execute(
                "INSERT INTO users (id, username, display_name, password_hash, role, created_at) VALUES ('u1','alex','Alex','x','admin',0)",
                [],
            )
        })
        .await
        .unwrap();
        assert!(recovery_key_needed(&db, "u1").await);
        let first = open_or_create(&db, "u1", "a long password")
            .await
            .unwrap()
            .unwrap();
        let shown = first.recovery_key.clone().unwrap();
        let again = open_or_create(&db, "u1", "a long password")
            .await
            .unwrap()
            .unwrap();
        assert!(again.recovery_key.is_none(), "shown only once");
        assert!(again.key.same_as(&first.key));
        assert!(
            open_or_create(&db, "u1", "the wrong password")
                .await
                .unwrap()
                .is_none()
        );
        let stored = load(&db, "u1").await.unwrap().unwrap();
        assert!(
            stored
                .open_with_recovery("u1", &RecoveryKey::parse(&shown).unwrap())
                .unwrap()
                .same_as(&first.key)
        );
        let replaced = new_recovery_key(&db, "u1", &first.key).await.unwrap();
        assert_ne!(replaced, shown);
        mark_saved(&db, "u1").await.unwrap();
        assert!(!recovery_key_needed(&db, "u1").await);
    }
}
