use crate::IpcError;
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Maximum frame size: 16MB.
const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

/// Write a length-prefixed JSON frame to an async writer.
///
/// # Frame Format
///
/// ```text
/// +----------------+------------------+
/// | Length (4 bytes) | JSON UTF-8 data |
/// |  little-endian  |                 |
/// +----------------+------------------+
/// ```
///
/// # Errors
///
/// Returns `IpcError::Json` if serialization fails.
/// Returns `IpcError::Io` if write fails.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &impl Serialize,
) -> Result<(), IpcError> {
    let json = serde_json::to_vec(value)?;
    let len = json.len();

    if len > MAX_FRAME_SIZE {
        return Err(IpcError::FrameTooLarge(len));
    }

    let len_bytes = (len as u32).to_le_bytes();
    writer.write_all(&len_bytes).await?;
    writer.write_all(&json).await?;
    writer.flush().await?;

    Ok(())
}

/// Read a length-prefixed JSON frame from an async reader.
///
/// # Errors
///
/// Returns `IpcError::ConnectionClosed` if EOF encountered before full frame read.
/// Returns `IpcError::FrameTooLarge` if frame size exceeds `MAX_FRAME_SIZE`.
/// Returns `IpcError::Json` if deserialization fails.
/// Returns `IpcError::Io` if read fails.
pub async fn read_frame<R: AsyncRead + Unpin, T: DeserializeOwned>(
    reader: &mut R,
) -> Result<T, IpcError> {
    let mut len_bytes = [0u8; 4];
    reader.read_exact(&mut len_bytes).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            IpcError::ConnectionClosed
        } else {
            IpcError::Io(e)
        }
    })?;

    let len = u32::from_le_bytes(len_bytes) as usize;

    if len > MAX_FRAME_SIZE {
        return Err(IpcError::FrameTooLarge(len));
    }

    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;

    let value = serde_json::from_slice(&buf)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct TestMessage {
        text: String,
        number: u64,
    }

    #[tokio::test]
    async fn write_frame_then_read_frame_round_trips_correctly() {
        let msg = TestMessage {
            text: "hello world".to_string(),
            number: 42,
        };

        let mut buf = Vec::new();
        write_frame(&mut buf, &msg).await.unwrap();

        let mut cursor = &buf[..];
        let decoded: TestMessage = read_frame(&mut cursor).await.unwrap();

        assert_eq!(decoded, msg);
    }

    #[tokio::test]
    async fn read_frame_returns_connection_closed_on_eof() {
        let empty: &[u8] = &[];
        let mut cursor = empty;

        let result: Result<TestMessage, _> = read_frame(&mut cursor).await;
        assert!(matches!(result, Err(IpcError::ConnectionClosed)));
    }

    #[tokio::test]
    async fn write_frame_rejects_oversized_payload() {
        let huge = "x".repeat(17 * 1024 * 1024);
        let msg = TestMessage {
            text: huge,
            number: 0,
        };

        let mut buf = Vec::new();
        let result = write_frame(&mut buf, &msg).await;

        assert!(matches!(result, Err(IpcError::FrameTooLarge(_))));
    }
}
