//! IPC codec: message framing and serialisation format.
//!
//! Decoupled from transport: JSON can run over Unix sockets or TCP.
//! Future: Protobuf codec (feature `ipc-protobuf`) with length-prefixed framing.

use crate::ipc::{IpcCommand, IpcResponse};
use serde_json;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

// ---------------------------------------------------------------------------
// JsonLineCodec — newline-delimited JSON (current default)
// ---------------------------------------------------------------------------

/// Newline-delimited JSON codec for IPC.
///
/// Framing: each message is a single line of JSON, terminated by `\n`.
/// Serialisation: `serde_json`.
///
/// This is the default codec. A future [`ProtobufCodec`] would use
/// length-prefixed binary framing + `prost` serialization.
#[derive(Clone, Copy)]
pub(crate) struct JsonLineCodec;

impl JsonLineCodec {
  /// Read one command from the stream.
  ///
  /// Reads a newline-delimited line, trims whitespace, ignores empty lines,
  /// and deserialises as [`IpcCommand`].
  pub(crate) async fn read_command<S: AsyncRead + Unpin>(
    &self,
    reader: &mut S,
  ) -> std::io::Result<IpcCommand> {
    let mut lines = BufReader::new(reader).lines();
    loop {
      let line = match lines.next_line().await? {
        Some(l) => l,
        None => {
          return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "IPC connection closed",
          ))
        }
      };
      let line = line.trim().to_string();
      if line.is_empty() {
        continue;
      }
      return serde_json::from_str(&line)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()));
    }
  }

  /// Write one response to the stream.
  ///
  /// Serialises as JSON, appends `\n`, writes to stream.
  pub(crate) async fn write_response<S: AsyncWrite + Unpin>(
    &self,
    writer: &mut S,
    resp: &IpcResponse,
  ) -> std::io::Result<()> {
    let mut json = serde_json::to_string(resp)
      .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    json.push('\n');
    writer.write_all(json.as_bytes()).await
  }
}
