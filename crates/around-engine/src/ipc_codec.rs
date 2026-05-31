//! IPC codec: message framing and serialisation format.
//!
//! Cross-platform — decoupled from transport.
//! JSON newline-delimited protocol. Future: Protobuf codec.

use crate::ipc_types::{IpcCommand, IpcResponse};
use serde_json;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

// ---------------------------------------------------------------------------
// JsonLineCodec — newline-delimited JSON (cross-platform)
// ---------------------------------------------------------------------------

/// Newline-delimited JSON codec for IPC.
///
/// Framing: each message is a single line of JSON, terminated by `\n`.
/// Serialisation: `serde_json`.
#[derive(Clone, Copy)]
pub(crate) struct JsonLineCodec;

impl JsonLineCodec {
  /// Read one command from the stream.
  pub(crate) async fn read_command<R: AsyncRead + Unpin>(
    &self,
    reader: &mut BufReader<R>,
  ) -> std::io::Result<IpcCommand> {
    loop {
      let mut line = String::new();
      let n = reader.read_line(&mut line).await?;
      if n == 0 {
        return Err(std::io::Error::new(
          std::io::ErrorKind::UnexpectedEof,
          "IPC connection closed",
        ));
      }
      let line = line.trim().to_string();
      if line.is_empty() {
        continue;
      }
      return serde_json::from_str(&line)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()));
    }
  }

  /// Write one response to the stream.
  pub(crate) async fn write_response<W: AsyncWrite + Unpin>(
    &self,
    writer: &mut W,
    resp: &IpcResponse,
  ) -> std::io::Result<()> {
    let mut json = serde_json::to_string(resp)
      .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    json.push('\n');
    writer.write_all(json.as_bytes()).await
  }
}
