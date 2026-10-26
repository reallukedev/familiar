//! Saved memory of encrypted Macs: hibernation files and live snapshots.
//!
//! QEMU's migrate-to-file writes plaintext, so for encrypted Macs QEMU migrates into a
//! pipe handed to it over QMP (`getfd`, then `fd:` URIs) and this streams the pipe
//! through AES-256-GCM in the STREAM construction (`aead`'s BE32: 1 MiB chunks, with a
//! counter and a last-chunk flag in each nonce, so chunks can't be reordered, dropped
//! or cut off). Each file gets its own key from the Mac's key and a random salt in its
//! header, and the header is authenticated with every chunk. Restoring runs it back:
//! each chunk is verified before any of it reaches QEMU.
use crate::keys::MacKey;
use aead::{
    Payload,
    generic_array::GenericArray,
    stream::{DecryptorBE32, EncryptorBE32},
};
use aes_gcm::{Aes256Gcm, KeyInit};
use anyhow::{Context, bail};
use std::io::{Read, Write};
use zeroize::Zeroizing;

/// File signature.
pub const MAGIC: &[u8; 8] = b"FAMMEM1\0";
const SALT: usize = 32;
const CHUNK: usize = 1 << 20;
const TAG: usize = 16;

fn cipher(key: &MacKey, salt: &[u8]) -> Aes256Gcm {
    let k = key.memory_key(salt);
    Aes256Gcm::new(GenericArray::from_slice(&k[..]))
}

/// Fill `buf` from `src`, stopping early only at the end of the stream.
fn read_full(src: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match src.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(m) => n += m,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Encrypt everything `src` gives until it closes into `dst`. Returns plaintext bytes.
pub fn encrypt(mut src: impl Read, mut dst: impl Write, key: &MacKey) -> anyhow::Result<u64> {
    let salt = dot_wire::random_bytes::<SALT>();
    let mut header = MAGIC.to_vec();
    header.extend_from_slice(&salt);
    dst.write_all(&header)?;
    let mut stream =
        EncryptorBE32::from_aead(cipher(key, &salt), GenericArray::from_slice(&[0u8; 7]));
    let mut buf = Zeroizing::new(vec![0u8; CHUNK]);
    let mut total = 0u64;
    loop {
        let n = read_full(&mut src, &mut buf).context("reading the Mac's memory")?;
        total += n as u64;
        // Full chunks are never the last: a stream that ends on a chunk boundary still
        // gets an empty final chunk, so the reader always knows where it ends.
        if n < CHUNK {
            let ct = stream
                .encrypt_last(Payload {
                    msg: &buf[..n],
                    aad: &header,
                })
                .map_err(|_| anyhow::anyhow!("encrypting the Mac's memory failed"))?;
            dst.write_all(&ct)?;
            break;
        }
        let ct = stream
            .encrypt_next(Payload {
                msg: &buf[..],
                aad: &header,
            })
            .map_err(|_| anyhow::anyhow!("encrypting the Mac's memory failed"))?;
        dst.write_all(&ct)?;
    }
    dst.flush()?;
    Ok(total)
}

/// Decrypt a file made by [`encrypt`] into `dst`. Fails, having written only verified
/// chunks, when the file was changed, cut short or made with another key.
pub fn decrypt(mut src: impl Read, mut dst: impl Write, key: &MacKey) -> anyhow::Result<u64> {
    let mut header = vec![0u8; MAGIC.len() + SALT];
    if read_full(&mut src, &mut header)? != header.len() || !header.starts_with(MAGIC) {
        bail!("the saved memory isn't an encrypted memory file");
    }
    let salt = header[MAGIC.len()..].to_vec();
    let mut stream =
        DecryptorBE32::from_aead(cipher(key, &salt), GenericArray::from_slice(&[0u8; 7]));
    let mut buf = vec![0u8; CHUNK + TAG];
    let mut total = 0u64;
    let bad = || anyhow::anyhow!("the saved memory doesn't match this Mac's key or was changed");
    loop {
        let n = read_full(&mut src, &mut buf)?;
        if n == buf.len() {
            let pt = Zeroizing::new(
                stream
                    .decrypt_next(Payload {
                        msg: &buf[..],
                        aad: &header,
                    })
                    .map_err(|_| bad())?,
            );
            dst.write_all(&pt)?;
            total += pt.len() as u64;
            continue;
        }
        if n < TAG {
            bail!("the saved memory is cut short");
        }
        let pt = Zeroizing::new(
            stream
                .decrypt_last(Payload {
                    msg: &buf[..n],
                    aad: &header,
                })
                .map_err(|_| bad())?,
        );
        dst.write_all(&pt)?;
        total += pt.len() as u64;
        break;
    }
    dst.flush()?;
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(b: u8) -> MacKey {
        MacKey::from_bytes([b; 32])
    }

    fn round_trip(len: usize) {
        let plain: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        let mut sealed = Vec::new();
        assert_eq!(
            encrypt(&plain[..], &mut sealed, &key(1)).unwrap(),
            len as u64
        );
        assert!(sealed.starts_with(MAGIC));
        if len >= 64 {
            assert!(
                !sealed.windows(64).any(|w| w == &plain[..64]),
                "no plaintext in the file"
            );
        }
        let mut back = Vec::new();
        decrypt(&sealed[..], &mut back, &key(1)).unwrap();
        assert_eq!(back, plain);
    }

    #[test]
    fn round_trips_any_length_including_chunk_boundaries() {
        for len in [
            0,
            1,
            4096,
            CHUNK - 1,
            CHUNK,
            CHUNK + 1,
            2 * CHUNK,
            2 * CHUNK + 7,
        ] {
            round_trip(len);
        }
    }

    #[test]
    fn refuses_the_wrong_key_changes_truncation_and_plain_files() {
        let plain = vec![7u8; CHUNK * 2 + 100];
        let mut sealed = Vec::new();
        encrypt(&plain[..], &mut sealed, &key(1)).unwrap();
        assert!(decrypt(&sealed[..], &mut Vec::new(), &key(2)).is_err());

        let mut flipped = sealed.clone();
        let i = flipped.len() / 2;
        flipped[i] ^= 1;
        assert!(decrypt(&flipped[..], &mut Vec::new(), &key(1)).is_err());

        // Dropping the final chunk (a cut at a chunk boundary) is caught.
        let header = MAGIC.len() + SALT;
        let cut = &sealed[..header + 2 * (CHUNK + TAG)];
        let mut out = Vec::new();
        assert!(decrypt(cut, &mut out, &key(1)).is_err());

        // A changed header (salt) fails every chunk.
        let mut salted = sealed.clone();
        salted[MAGIC.len()] ^= 1;
        assert!(decrypt(&salted[..], &mut Vec::new(), &key(1)).is_err());

        assert!(
            decrypt(
                &b"QEVM plain migration stream"[..],
                &mut Vec::new(),
                &key(1)
            )
            .is_err()
        );
    }
}
