//! Argon2id password hashing (OWASP parameters), off the async threads.
use argon2::{
    Algorithm, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier, Version,
    password_hash::SaltString,
};
use std::sync::LazyLock;

fn hasher() -> Argon2<'static> {
    // 19 MiB, 2 passes, 1 lane: OWASP's recommended Argon2id minimum.
    let params = Params::new(19 * 1024, 2, 1, None).unwrap_or_default();
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Verified against when the username doesn't exist, so timing doesn't reveal accounts.
static DUMMY: LazyLock<String> =
    LazyLock::new(|| hash_blocking("dot-home-timing-equalizer").unwrap_or_default());

fn hash_blocking(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::encode_b64(&dot_wire::random_bytes::<16>())
        .map_err(|e| anyhow::anyhow!("salt: {e}"))?;
    Ok(hasher()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("hash: {e}"))?
        .to_string())
}

/// Hash a new password.
pub async fn hash(password: String) -> anyhow::Result<String> {
    tokio::task::spawn_blocking(move || hash_blocking(&password)).await?
}

/// Check a password against a stored hash; `None` runs a dummy check and returns false.
pub async fn verify(password: String, stored: Option<String>) -> bool {
    tokio::task::spawn_blocking(move || {
        let stored = stored.unwrap_or_else(|| DUMMY.clone());
        PasswordHash::new(&stored)
            .map(|h| hasher().verify_password(password.as_bytes(), &h).is_ok())
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

/// Policy: 10 to 256 characters, not all one character, not the username.
pub fn check_policy(password: &str, username: &str) -> Result<(), &'static str> {
    let len = password.chars().count();
    if len < 10 {
        return Err("Use at least 10 characters.");
    }
    if len > 256 {
        return Err("Use at most 256 characters.");
    }
    let mut chars = password.chars();
    if let Some(first) = chars.next()
        && chars.all(|c| c == first)
    {
        return Err("Use more than one repeated character.");
    }
    if password.eq_ignore_ascii_case(username) {
        return Err("Your password can't be your username.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hashes_verify_and_unknown_users_fail() {
        let h = hash("correct horse battery".into()).await.unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify("correct horse battery".into(), Some(h.clone())).await);
        assert!(!verify("wrong horse battery".into(), Some(h)).await);
        assert!(!verify("anything".into(), None).await);
        assert!(!verify("x".into(), Some("not a hash".into())).await);
    }

    #[test]
    fn policy() {
        assert!(check_policy("short", "u").is_err());
        assert!(check_policy("aaaaaaaaaaaa", "u").is_err());
        assert!(check_policy("alexander10", "Alexander10").is_err());
        assert!(check_policy("a perfectly fine phrase", "alex").is_ok());
    }
}
