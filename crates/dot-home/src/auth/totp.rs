//! RFC 6238 TOTP (SHA-1, 6 digits, 30 s), as every authenticator app expects.
use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, Mac};
use sha1::Sha1;

const STEP: i64 = 30;

/// A fresh 160-bit secret.
pub fn new_secret() -> Vec<u8> {
    dot_wire::random_bytes::<20>().to_vec()
}

/// Base32 for manual entry.
pub fn encode(secret: &[u8]) -> String {
    BASE32_NOPAD.encode(secret)
}

/// `otpauth://` URL for QR codes.
pub fn otpauth_url(secret: &[u8], issuer: &str, account: &str) -> String {
    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits=6&period=30",
        enc(issuer),
        enc(account),
        encode(secret),
        enc(issuer)
    )
}

fn code_at(secret: &[u8], step: i64) -> u32 {
    let Ok(mut mac) = <Hmac<Sha1> as Mac>::new_from_slice(secret) else {
        return u32::MAX;
    };
    mac.update(&step.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let offset = (h[19] & 0x0f) as usize;
    let bin = (u32::from(h[offset]) & 0x7f) << 24
        | u32::from(h[offset + 1]) << 16
        | u32::from(h[offset + 2]) << 8
        | u32::from(h[offset + 3]);
    bin % 1_000_000
}

/// The 6-digit code for `unix` time (for clients and tests).
pub fn code(secret: &[u8], unix: i64) -> String {
    format!("{:06}", code_at(secret, unix.div_euclid(STEP)))
}

/// Check `code` at `now` allowing one step of clock drift either way. Returns the
/// matched step, which must be greater than `last_step` so a code can't be replayed.
pub fn verify(secret: &[u8], code: &str, now: i64, last_step: i64) -> Option<i64> {
    let code = code.trim().replace(' ', "");
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let want: u32 = code.parse().ok()?;
    let current = now.div_euclid(STEP);
    [current - 1, current, current + 1]
        .into_iter()
        .filter(|&s| s > last_step)
        .find(|&s| {
            let got = code_at(secret, s);
            subtle::ConstantTimeEq::ct_eq(&got.to_be_bytes()[..], &want.to_be_bytes()[..]).into()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 6238 appendix B, SHA-1 seed, truncated to 6 digits.
    const SEED: &[u8] = b"12345678901234567890";

    #[test]
    fn rfc_vectors_drift_and_replay() {
        assert_eq!(code_at(SEED, 59 / 30), 287_082);
        assert_eq!(code_at(SEED, 1_111_111_109 / 30), 81_804);
        assert_eq!(code_at(SEED, 1_234_567_890 / 30), 5_924);
        assert_eq!(verify(SEED, "287082", 59, 0), Some(1));
        assert_eq!(
            verify(SEED, "287 082", 59 + 30, 0),
            Some(1),
            "one step late"
        );
        assert_eq!(verify(SEED, "287082", 59 + 90, 0), None, "too late");
        assert_eq!(verify(SEED, "287082", 59, 1), None, "replayed");
        assert_eq!(verify(SEED, "28708", 59, 0), None);
        assert_eq!(verify(SEED, "abcdef", 59, 0), None);
        assert!(otpauth_url(SEED, "Familiar", "alex").contains("issuer=Familiar"));
    }
}
