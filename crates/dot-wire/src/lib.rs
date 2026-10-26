//! The protocol between the Dot home server (host) and `dot-guest`, the agent that runs
//! inside each Mac.
//!
//! Transport: one TCP stream per Mac, opened by the host. Every frame is a 4-byte
//! big-endian length followed by that many bytes of JSON. The first three frames are a
//! mutual HMAC handshake over a per-instance secret ([`handshake`]); after that the host
//! sends [`Envelope`]s and the guest answers each with a [`Reply`] carrying the same id,
//! in any order, so slow calls never block fast ones.
pub mod frame;
pub mod handshake;
mod ops;

pub use ops::*;

/// Bumped when a change is not backward compatible. The host refuses other majors.
pub const PROTOCOL_MAJOR: u32 = 1;
/// Largest frame either side accepts (screenshots and file reads dominate).
pub const MAX_FRAME: usize = 48 * 1024 * 1024;
/// TCP port the guest listens on inside the Mac.
pub const GUEST_PORT: u16 = 7466;
/// Volume label of the per-instance configuration disk the host attaches.
pub const CONFIG_VOLUME: &str = "DOTCONFIG";

/// Generate `N` random bytes from the OS.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    // getrandom only fails when the OS has no entropy source at all, which is fatal anyway.
    if getrandom::fill(&mut out).is_err() {
        std::process::abort();
    }
    out
}
