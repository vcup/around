//! IPC codec: message framing and serialisation format.
//!
//! Transport-agnostic codec abstraction. All transports use the same
//! `IpcCodec` trait for reading commands and writing responses.
//! Async-only — all transports (including Windows named pipes) use tokio async I/O.

use super::types::{IpcCommand, IpcResponse};
#[cfg(feature = "json-ipc")]
use serde_json;
#[cfg(feature = "json-ipc")]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::io::{AsyncRead, AsyncWrite, BufReader};

/// Transport-agnostic IPC codec.
///
/// Implementations provide wire format framing and serialisation.
/// All transports use the same async interface — no sync variants needed.
pub(crate) trait IpcCodec {
  async fn read_command<R: AsyncRead + Unpin>(
    &self,
    reader: &mut BufReader<R>,
  ) -> std::io::Result<IpcCommand>;

  async fn write_response<W: AsyncWrite + Unpin>(
    &self,
    writer: &mut W,
    resp: &IpcResponse,
  ) -> std::io::Result<()>;
}

/// Runtime-selectable wire format — concrete enum (Send-safe for tokio::spawn).
///
/// Protobuf is preferred when available; JSON is the fallback.
/// The `force_json` flag selects JSON even when both are compiled.
#[derive(Clone)]
pub(crate) enum IpcWire {
  #[cfg(feature = "protobuf-ipc")]
  Proto(super::codec_proto::ProtoCodec),
  #[cfg(feature = "json-ipc")]
  Json(JsonLineCodec),
}

impl IpcWire {
  pub(crate) fn select(force_json: bool) -> Self {
    #[cfg(all(feature = "protobuf-ipc", feature = "json-ipc"))]
    {
      if force_json {
        return IpcWire::Json(JsonLineCodec);
      }
      IpcWire::Proto(super::codec_proto::ProtoCodec)
    }
    #[cfg(all(feature = "protobuf-ipc", not(feature = "json-ipc")))]
    {
      // Protobuf-only build: no JSON fallback available.
      return IpcWire::Proto(super::codec_proto::ProtoCodec);
    }
    #[cfg(all(feature = "json-ipc", not(feature = "protobuf-ipc")))]
    {
      return IpcWire::Json(JsonLineCodec);
    }
    #[cfg(not(any(feature = "protobuf-ipc", feature = "json-ipc")))]
    {
      compile_error!("at least one IPC codec feature must be enabled");
    }
  }
}

impl IpcCodec for IpcWire {
  async fn read_command<R: AsyncRead + Unpin>(
    &self,
    reader: &mut BufReader<R>,
  ) -> std::io::Result<IpcCommand> {
    match self {
      #[cfg(feature = "protobuf-ipc")]
      IpcWire::Proto(c) => c.read_command(reader).await,
      #[cfg(feature = "json-ipc")]
      IpcWire::Json(c) => c.read_command(reader).await,
    }
  }
  async fn write_response<W: AsyncWrite + Unpin>(
    &self,
    writer: &mut W,
    resp: &IpcResponse,
  ) -> std::io::Result<()> {
    match self {
      #[cfg(feature = "protobuf-ipc")]
      IpcWire::Proto(c) => c.write_response(writer, resp).await,
      #[cfg(feature = "json-ipc")]
      IpcWire::Json(c) => c.write_response(writer, resp).await,
    }
  }
}

// ---------------------------------------------------------------------------
// JsonLineCodec — newline-delimited JSON (cross-platform)
// ---------------------------------------------------------------------------

/// Newline-delimited JSON codec (fallback — gated behind `json-ipc` feature).
#[cfg(feature = "json-ipc")]
#[derive(Clone, Copy)]
pub(crate) struct JsonLineCodec;

#[cfg(feature = "json-ipc")]
impl IpcCodec for JsonLineCodec {
  async fn read_command<R: AsyncRead + Unpin>(
    &self,
    reader: &mut BufReader<R>,
  ) -> std::io::Result<IpcCommand> {
    const MAX_LINE_LEN: usize = 64 * 1024;
    let mut line = String::new();
    let mut skipped: usize = 0;
    loop {
      line.clear();
      let n = reader.read_line(&mut line).await?;
      if n == 0 {
        return Err(std::io::Error::new(
          std::io::ErrorKind::UnexpectedEof,
          "IPC connection closed",
        ));
      }
      if line.len() > MAX_LINE_LEN {
        return Err(std::io::Error::new(
          std::io::ErrorKind::InvalidData,
          "IPC line exceeds maximum length",
        ));
      }
      let trimmed = line.trim();
      if trimmed.is_empty() {
        skipped += 1;
        if skipped > 16 {
          return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "too many empty lines",
          ));
        }
        continue;
      }
      return serde_json::from_str(trimmed)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e));
    }
  }

  async fn write_response<W: AsyncWrite + Unpin>(
    &self,
    writer: &mut W,
    resp: &IpcResponse,
  ) -> std::io::Result<()> {
    let mut json = serde_json::to_string(resp)
      .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    json.push('\n');
    writer.write_all(json.as_bytes()).await
  }
}

#[cfg(all(test, feature = "json-ipc"))]
mod tests {
  use super::*;
  use crate::ipc::types::ErrorCode;
  use std::io::Cursor;
  use tokio::io::BufReader;

  fn make_reader(data: &[u8]) -> BufReader<Cursor<Vec<u8>>> {
    BufReader::new(Cursor::new(data.to_vec()))
  }

  #[expect(
    clippy::unwrap_used,
    reason = "in-memory Cursor with valid JSON always produces Ok for read_command"
  )]
  #[tokio::test]
  async fn parse_valid_json_command() {
    let codec = JsonLineCodec;
    let mut reader = make_reader(b"{\"command\":\"status\"}\n");
    let cmd = codec.read_command(&mut reader).await.unwrap();
    assert!(matches!(cmd, IpcCommand::Status { .. }));
  }

  #[expect(
    clippy::unwrap_used,
    reason = "known valid JSON after empty lines in controlled Cursor input"
  )]
  #[tokio::test]
  async fn skip_empty_lines() {
    let codec = JsonLineCodec;
    let mut reader = make_reader(b"\n\n{\"command\":\"pause\"}\n");
    let cmd = codec.read_command(&mut reader).await.unwrap();
    assert!(matches!(cmd, IpcCommand::Pause { .. }));
  }

  #[expect(
    clippy::unwrap_used,
    reason = "whitespace-only lines do not prevent parsing subsequent valid JSON in Cursor"
  )]
  #[tokio::test]
  async fn skip_whitespace_only_lines() {
    let codec = JsonLineCodec;
    let mut reader = make_reader(b"   \n{\"command\":\"resume\"}\n");
    let cmd = codec.read_command(&mut reader).await.unwrap();
    assert!(matches!(cmd, IpcCommand::Resume { .. }));
  }

  #[expect(
    clippy::unwrap_used,
    reason = "deliberately invalid JSON must produce Err from in-memory Cursor"
  )]
  #[tokio::test]
  async fn invalid_json_returns_error() {
    let codec = JsonLineCodec;
    let mut reader = make_reader(b"not json\n");
    let err = codec.read_command(&mut reader).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
  }

  #[expect(
    clippy::unwrap_used,
    reason = "empty Cursor by design produces UnexpectedEof from read_command"
  )]
  #[tokio::test]
  async fn eof_returns_unexpected_eof() {
    let codec = JsonLineCodec;
    let mut reader = make_reader(b"");
    let err = codec.read_command(&mut reader).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
  }

  #[expect(
    clippy::unwrap_used,
    reason = "valid JSON without trailing newline in Cursor is still parseable"
  )]
  #[tokio::test]
  async fn valid_json_without_trailing_newline() {
    let codec = JsonLineCodec;
    let mut reader = make_reader(b"{\"command\":\"status\"}");
    let cmd = codec.read_command(&mut reader).await.unwrap();
    assert!(matches!(cmd, IpcCommand::Status { .. }));
  }

  #[expect(
    clippy::unwrap_used,
    reason = "only blank lines then EOF produces UnexpectedEof by design"
  )]
  #[tokio::test]
  async fn only_empty_lines_then_eof_returns_error() {
    let codec = JsonLineCodec;
    let mut reader = make_reader(b"\n\n\n");
    let err = codec.read_command(&mut reader).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
  }

  #[expect(
    clippy::unwrap_used,
    reason = "only whitespace lines then EOF produces UnexpectedEof by design"
  )]
  #[tokio::test]
  async fn only_whitespace_lines_then_eof() {
    let codec = JsonLineCodec;
    let mut reader = make_reader(b"  \n\t\n");
    let err = codec.read_command(&mut reader).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
  }

  #[expect(
    clippy::unwrap_used,
    reason = "valid JSON with surrounding whitespace in Cursor still parses correctly"
  )]
  #[tokio::test]
  async fn json_with_leading_trailing_whitespace() {
    let codec = JsonLineCodec;
    let mut reader = make_reader(b"  {\"command\":\"stop\"}  \n");
    let cmd = codec.read_command(&mut reader).await.unwrap();
    assert!(matches!(cmd, IpcCommand::Stop { .. }));
  }

  #[expect(
    clippy::unwrap_used,
    reason = "valid JSON with tabs around it in Cursor still parses correctly"
  )]
  #[tokio::test]
  async fn json_with_tabs_and_newlines_around() {
    let codec = JsonLineCodec;
    let mut reader = make_reader(b"\t{\"command\":\"pause\"}\t\n");
    let cmd = codec.read_command(&mut reader).await.unwrap();
    assert!(matches!(cmd, IpcCommand::Pause { .. }));
  }

  #[expect(
    clippy::unwrap_used,
    reason = "two valid JSON commands separated by blank lines in Cursor both parse"
  )]
  #[tokio::test]
  async fn command_followed_by_empty_lines_read_on_next_call() {
    let codec = JsonLineCodec;
    let mut reader = make_reader(b"{\"command\":\"stop\"}\n\n\n{\"command\":\"pause\"}\n");

    let cmd1 = codec.read_command(&mut reader).await.unwrap();
    assert!(matches!(cmd1, IpcCommand::Stop { .. }));

    let cmd2 = codec.read_command(&mut reader).await.unwrap();
    assert!(matches!(cmd2, IpcCommand::Pause { .. }));
  }

  #[expect(
    clippy::unwrap_used,
    reason = "too many empty lines in Cursor by design produces InvalidData error"
  )]
  #[tokio::test]
  async fn too_many_empty_lines_returns_error() {
    let codec = JsonLineCodec;
    let data = vec![b'\n'; 17];
    let mut reader = make_reader(&data);
    let err = codec.read_command(&mut reader).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("too many empty lines"));
  }

  #[expect(
    clippy::unwrap_used,
    reason = "valid JSON after exactly 16 empty lines is within Cursor limit"
  )]
  #[tokio::test]
  async fn exactly_sixteen_empty_lines_then_command_succeeds() {
    let codec = JsonLineCodec;
    let mut data = vec![b'\n'; 16];
    data.extend_from_slice(b"{\"command\":\"stop\"}\n");
    let mut reader = make_reader(&data);
    let cmd = codec.read_command(&mut reader).await.unwrap();
    assert!(matches!(cmd, IpcCommand::Stop { .. }));
  }

  #[expect(
    clippy::unwrap_used,
    reason = "Vec<u8> writer never fails; JSON output is valid UTF-8 and re-parses"
  )]
  #[tokio::test]
  async fn write_response_adds_newline() {
    let codec = JsonLineCodec;
    let mut buf = Vec::new();
    let resp = IpcResponse::ok();
    codec.write_response(&mut buf, &resp).await.unwrap();
    let text = String::from_utf8(buf).unwrap();
    assert!(text.ends_with('\n'), "response must end with newline");
    let parsed: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(parsed["status"], "ok");
  }

  #[expect(
    clippy::unwrap_used,
    reason = "Vec<u8> writer never fails; JSON output is valid UTF-8 and re-parses"
  )]
  #[tokio::test]
  async fn write_response_error_during_play() {
    let codec = JsonLineCodec;
    let mut buf = Vec::new();
    let resp = IpcResponse::error(ErrorCode::NoTrack, "no active playback");
    codec.write_response(&mut buf, &resp).await.unwrap();
    let text = String::from_utf8(buf).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(parsed["status"], "error");
    assert_eq!(parsed["code"], "NO_TRACK");
  }

  #[expect(
    clippy::unwrap_used,
    reason = "controlled play command JSON in Cursor always produces Ok"
  )]
  #[tokio::test]
  async fn parse_play_command_with_path() {
    let codec = JsonLineCodec;
    let mut reader = make_reader(b"{\"command\":\"play\",\"path\":\"/tmp/test.wav\"}\n");
    let cmd = codec.read_command(&mut reader).await.unwrap();
    match cmd {
      IpcCommand::Play { path, .. } => assert_eq!(path, "/tmp/test.wav"),
      _ => panic!("expected Play"),
    }
  }

  #[expect(
    clippy::unwrap_used,
    reason = "controlled seek command JSON in Cursor always produces Ok"
  )]
  #[tokio::test]
  async fn parse_seek_command_with_position() {
    let codec = JsonLineCodec;
    let mut reader = make_reader(b"{\"command\":\"seek\",\"position_ms\":5000}\n");
    let cmd = codec.read_command(&mut reader).await.unwrap();
    match cmd {
      IpcCommand::Seek { position_ms, .. } => assert_eq!(position_ms, 5000),
      _ => panic!("expected Seek"),
    }
  }

  #[expect(
    clippy::unwrap_used,
    reason = "controlled load_codec_bytes JSON in Cursor always produces Ok"
  )]
  #[tokio::test]
  async fn parse_load_codec_bytes_command() {
    let codec = JsonLineCodec;
    let mut reader = make_reader(b"{\"command\":\"load_codec_bytes\",\"data\":\"dGVzdA==\"}\n");
    let cmd = codec.read_command(&mut reader).await.unwrap();
    match cmd {
      IpcCommand::LoadCodecBytes { data } => {
        assert_eq!(data, b"test");
      }
      _ => panic!("expected LoadCodecBytes"),
    }
  }

  #[expect(
    clippy::unwrap_used,
    reason = "three sequentially valid JSON commands in Cursor all parse correctly"
  )]
  #[tokio::test]
  async fn multiple_commands_in_sequence() {
    let codec = JsonLineCodec;
    let mut reader =
      make_reader(b"{\"command\":\"status\"}\n{\"command\":\"status\"}\n{\"command\":\"pause\"}\n");

    let cmd1 = codec.read_command(&mut reader).await.unwrap();
    assert!(matches!(cmd1, IpcCommand::Status { .. }));

    let cmd2 = codec.read_command(&mut reader).await.unwrap();
    assert!(matches!(cmd2, IpcCommand::Status { .. }));

    let cmd3 = codec.read_command(&mut reader).await.unwrap();
    assert!(matches!(cmd3, IpcCommand::Pause { .. }));
  }

  #[tokio::test]
  async fn error_response_includes_code_and_message() {
    let codec = JsonLineCodec;
    let mut buf = Vec::new();
    let resp = IpcResponse::error(ErrorCode::FileNotFound, "No such file: /tmp/missing.wav");
    #[expect(
      clippy::unwrap_used,
      reason = "Vec<u8> writer never fails for write_response"
    )]
    codec.write_response(&mut buf, &resp).await.unwrap();
    #[expect(
      clippy::unwrap_used,
      reason = "write_response serializes valid UTF-8 JSON"
    )]
    let text = String::from_utf8(buf).unwrap();
    #[expect(
      clippy::unwrap_used,
      reason = "write_response output is always valid JSON and deserializes on first parse"
    )]
    let parsed: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(parsed["status"], "error");
    assert_eq!(parsed["code"], "FILE_NOT_FOUND");
    assert_eq!(parsed["message"], "No such file: /tmp/missing.wav");
    assert!(
      parsed.get("state").is_none(),
      "error response must not have state"
    );
    assert!(
      parsed.get("position_ms").is_none(),
      "error response must not have position_ms"
    );
    assert!(
      parsed.get("track").is_none(),
      "error response must not have track"
    );
  }
}
