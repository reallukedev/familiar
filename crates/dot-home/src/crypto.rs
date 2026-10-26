//! Secrets: random tokens, hashing, and sealing small values with the master key.
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use anyhow::Context;
use data_encoding::BASE32_NOPAD;
use sha2::{Digest, Sha256};
use std::{io::Write, path::Path};

/// The key that seals TOTP secrets and guest secrets at rest.
#[derive(Clone)]
pub struct MasterKey(Aes256Gcm);

impl MasterKey {
    /// Load `master.key` from the data directory, creating it (0600) on first run.
    /// `DOT_HOME_MASTER_KEY_FILE` (e.g. a systemd or Docker secret) wins when set.
    pub fn load_or_create(data_dir: &Path) -> anyhow::Result<Self> {
        let path = std::env::var_os("DOT_HOME_MASTER_KEY_FILE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| data_dir.join("master.key"));
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let key = dot_wire::random_bytes::<32>();
                write_private(&path, &key)?;
                key.to_vec()
            }
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        anyhow::ensure!(
            bytes.len() == 32,
            "{} must be exactly 32 bytes",
            path.display()
        );
        Self::from_bytes(&bytes)
    }

    /// From raw bytes (tests).
    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        Ok(Self(
            Aes256Gcm::new_from_slice(bytes).map_err(|_| anyhow::anyhow!("bad key length"))?,
        ))
    }

    /// Encrypt `plain`, binding it to `context` (e.g. `"totp:<user id>"`) so a sealed
    /// value can't be moved to another row.
    pub fn seal(&self, context: &str, plain: &[u8]) -> Vec<u8> {
        let nonce = dot_wire::random_bytes::<12>();
        let mut out = nonce.to_vec();
        // Encryption with a valid key and a fresh nonce can't fail.
        if let Ok(ct) = self.0.encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plain,
                aad: context.as_bytes(),
            },
        ) {
            out.extend(ct);
        }
        out
    }

    /// Decrypt a value sealed with the same `context`.
    pub fn open(&self, context: &str, sealed: &[u8]) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(sealed.len() > 12, "sealed value too short");
        let (nonce, ct) = sealed.split_at(12);
        self.0
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ct,
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| anyhow::anyhow!("sealed value could not be opened"))
    }
}

/// Write a file readable only by its owner, atomically.
pub fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// SHA-256.
pub fn sha256(data: &[u8]) -> Vec<u8> {
    Sha256::digest(data).to_vec()
}

/// A random lowercase base32 id of `bytes` entropy.
pub fn random_id<const N: usize>() -> String {
    BASE32_NOPAD
        .encode(&dot_wire::random_bytes::<N>())
        .to_ascii_lowercase()
}

/// A bearer secret with a recognizable prefix, e.g. `dot_pat_…`. 32 bytes of entropy.
pub fn token(prefix: &str) -> String {
    format!("{prefix}{}", random_id::<32>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_values_are_bound_to_their_context_and_key() {
        let key = MasterKey::from_bytes(&[7; 32]).unwrap();
        let sealed = key.seal("totp:u1", b"secret");
        assert_eq!(key.open("totp:u1", &sealed).unwrap(), b"secret");
        assert!(key.open("totp:u2", &sealed).is_err());
        let other = MasterKey::from_bytes(&[8; 32]).unwrap();
        assert!(other.open("totp:u1", &sealed).is_err());
        assert_ne!(
            sealed,
            key.seal("totp:u1", b"secret"),
            "fresh nonce each time"
        );
    }

    #[test]
    fn master_key_is_created_private_and_reused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let a = MasterKey::load_or_create(dir.path()).unwrap();
        let mode = std::fs::metadata(dir.path().join("master.key"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let b = MasterKey::load_or_create(dir.path()).unwrap();
        assert_eq!(b.open("x", &a.seal("x", b"y")).unwrap(), b"y");
        assert!(token("dot_pat_").starts_with("dot_pat_"));
        assert_eq!(random_id::<10>().len(), 16);
    }
}
