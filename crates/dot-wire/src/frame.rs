//! Length-prefixed JSON frames.
use crate::MAX_FRAME;
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Why a frame could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// The peer closed the stream cleanly between frames.
    #[error("connection closed")]
    Closed,
    /// A frame claimed to be larger than [`MAX_FRAME`].
    #[error("frame of {0} bytes is over the limit")]
    TooLarge(usize),
    /// Socket failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The frame was not the JSON we expected.
    #[error("malformed frame: {0}")]
    Json(#[from] serde_json::Error),
}

/// Write one frame and flush it.
pub async fn write<W, T>(writer: &mut W, value: &T) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
    T: Serialize + ?Sized,
{
    let body = serde_json::to_vec(value)?;
    if body.len() > MAX_FRAME {
        return Err(FrameError::TooLarge(body.len()));
    }
    let len = u32::try_from(body.len()).map_err(|_| FrameError::TooLarge(body.len()))?;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}

/// Read one frame. A clean EOF before the length prefix is [`FrameError::Closed`].
pub async fn read<R, T>(reader: &mut R) -> Result<T, FrameError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len = [0u8; 4];
    match reader.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(FrameError::Closed),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(FrameError::TooLarge(len));
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[tokio::test]
    async fn round_trips_and_rejects_oversized_prefix() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        write(&mut a, &json!({"hello": "world"})).await.unwrap();
        let got: Value = read(&mut b).await.unwrap();
        assert_eq!(got, json!({"hello": "world"}));

        tokio::io::AsyncWriteExt::write_all(&mut a, &(u32::MAX).to_be_bytes())
            .await
            .unwrap();
        assert!(matches!(
            read::<_, Value>(&mut b).await,
            Err(FrameError::TooLarge(_))
        ));

        drop(a);
        let (c, mut d) = tokio::io::duplex(16);
        drop(c);
        assert!(matches!(
            read::<_, Value>(&mut d).await,
            Err(FrameError::Closed)
        ));
    }
}
