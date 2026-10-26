//! Mutual authentication over a shared per-instance secret.
//!
//! ```text
//! guest -> host  Hello   { protocol, agent_version, instance_id, guest_nonce }
//! host  -> guest Prove   { host_nonce, host_proof = HMAC(secret, "host"  | guest_nonce | host_nonce) }
//! guest -> host  Proven  { guest_proof = HMAC(secret, "guest" | host_nonce | guest_nonce), info }
//! ```
//!
//! Each side proves it knows the secret over a nonce the other side chose, so a
//! recorded session can't be replayed and neither side reveals the secret. The guest
//! speaks first so a host that dialed the wrong port learns it immediately.
use crate::{
    GuestInfo,
    frame::{self, FrameError},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use tokio::io::{AsyncRead, AsyncWrite};

type HmacSha256 = Hmac<Sha256>;

/// First frame, from the guest.
#[derive(Debug, Serialize, Deserialize)]
pub struct Hello {
    /// [`crate::PROTOCOL_MAJOR`] of the guest.
    pub protocol: u32,
    /// The guest binary's version.
    pub agent_version: String,
    /// Which instance this guest was provisioned for.
    pub instance_id: String,
    /// Base64 of 32 random bytes.
    pub guest_nonce: String,
}

/// Second frame, from the host.
#[derive(Debug, Serialize, Deserialize)]
pub struct Prove {
    /// Base64 of 32 random bytes.
    pub host_nonce: String,
    /// Base64 HMAC.
    pub host_proof: String,
}

/// Third frame, from the guest.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Proven {
    /// The host proved itself; here is the guest's proof and a description of the Mac.
    Ok {
        /// Base64 HMAC.
        guest_proof: String,
        /// The Mac this agent runs on.
        info: Box<GuestInfo>,
    },
    /// The host's proof was wrong; the guest closes the stream.
    Rejected,
}

/// Why a handshake failed.
#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    /// Framing or socket failure.
    #[error(transparent)]
    Frame(#[from] FrameError),
    /// The other side does not know the secret.
    #[error("the other side could not prove it knows this instance's secret")]
    BadProof,
    /// The guest belongs to a different instance.
    #[error("the agent was provisioned for instance {0}")]
    WrongInstance(String),
    /// Incompatible protocol major.
    #[error("agent speaks protocol {0}, host speaks {1}")]
    Protocol(u32, u32),
    /// Base64 in a nonce or proof was invalid.
    #[error("malformed handshake field")]
    Malformed,
}

fn proof(secret: &[u8], label: &[u8], first: &[u8], second: &[u8]) -> Vec<u8> {
    // HMAC accepts keys of any length.
    let mut mac = <HmacSha256 as Mac>::new_from_slice(secret).unwrap_or_else(|_| unreachable!());
    mac.update(label);
    mac.update(first);
    mac.update(second);
    mac.finalize().into_bytes().to_vec()
}

fn decode(value: &str) -> Result<Vec<u8>, HandshakeError> {
    STANDARD
        .decode(value)
        .map_err(|_| HandshakeError::Malformed)
}

fn verify(expected: &[u8], got: &str) -> Result<(), HandshakeError> {
    let got = decode(got)?;
    if expected.ct_eq(&got).into() {
        Ok(())
    } else {
        Err(HandshakeError::BadProof)
    }
}

/// Run the host side. Returns the guest's description of itself.
pub async fn host<S>(
    stream: &mut S,
    secret: &[u8],
    instance_id: &str,
) -> Result<(Hello, GuestInfo), HandshakeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let hello: Hello = frame::read(stream).await?;
    if hello.protocol != crate::PROTOCOL_MAJOR {
        return Err(HandshakeError::Protocol(
            hello.protocol,
            crate::PROTOCOL_MAJOR,
        ));
    }
    if hello.instance_id != instance_id {
        return Err(HandshakeError::WrongInstance(hello.instance_id));
    }
    let guest_nonce = decode(&hello.guest_nonce)?;
    let host_nonce = crate::random_bytes::<32>();
    let host_proof = proof(secret, b"host", &guest_nonce, &host_nonce);
    frame::write(
        stream,
        &Prove {
            host_nonce: STANDARD.encode(host_nonce),
            host_proof: STANDARD.encode(host_proof),
        },
    )
    .await?;
    match frame::read::<_, Proven>(stream).await? {
        Proven::Ok { guest_proof, info } => {
            verify(
                &proof(secret, b"guest", &host_nonce, &guest_nonce),
                &guest_proof,
            )?;
            Ok((hello, *info))
        }
        Proven::Rejected => Err(HandshakeError::BadProof),
    }
}

/// Run the guest side. `info` is only sent once the host has proved itself.
pub async fn guest<S>(
    stream: &mut S,
    secret: &[u8],
    instance_id: &str,
    agent_version: &str,
    info: impl FnOnce() -> GuestInfo,
) -> Result<(), HandshakeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let guest_nonce = crate::random_bytes::<32>();
    frame::write(
        stream,
        &Hello {
            protocol: crate::PROTOCOL_MAJOR,
            agent_version: agent_version.to_owned(),
            instance_id: instance_id.to_owned(),
            guest_nonce: STANDARD.encode(guest_nonce),
        },
    )
    .await?;
    let prove: Prove = frame::read(stream).await?;
    let host_nonce = decode(&prove.host_nonce)?;
    if let Err(e) = verify(
        &proof(secret, b"host", &guest_nonce, &host_nonce),
        &prove.host_proof,
    ) {
        let _ = frame::write(stream, &Proven::Rejected).await;
        return Err(e);
    }
    let guest_proof = proof(secret, b"guest", &host_nonce, &guest_nonce);
    frame::write(
        stream,
        &Proven::Ok {
            guest_proof: STANDARD.encode(guest_proof),
            info: Box::new(info()),
        },
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> GuestInfo {
        GuestInfo {
            os_version: "26.0".into(),
            hostname: "test-mac".into(),
            user: "dot".into(),
            ..GuestInfo::default()
        }
    }

    #[tokio::test]
    async fn matching_secrets_authenticate_both_ways() {
        let (mut h, mut g) = tokio::io::duplex(4096);
        let guest =
            tokio::spawn(async move { guest(&mut g, b"shared", "inst1", "1.0.0", info).await });
        let (hello, got) = host(&mut h, b"shared", "inst1").await.unwrap();
        assert_eq!(hello.agent_version, "1.0.0");
        assert_eq!(got.hostname, "test-mac");
        guest.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn wrong_secret_or_instance_is_refused_on_both_sides() {
        let (mut h, mut g) = tokio::io::duplex(4096);
        let guest_task =
            tokio::spawn(async move { guest(&mut g, b"guest-secret", "inst1", "1", info).await });
        assert!(matches!(
            host(&mut h, b"host-secret", "inst1").await,
            Err(HandshakeError::BadProof)
        ));
        assert!(matches!(
            guest_task.await.unwrap(),
            Err(HandshakeError::BadProof)
        ));

        let (mut h, mut g) = tokio::io::duplex(4096);
        tokio::spawn(async move { guest(&mut g, b"s", "other", "1", info).await });
        assert!(matches!(
            host(&mut h, b"s", "inst1").await,
            Err(HandshakeError::WrongInstance(id)) if id == "other"
        ));
    }
}
