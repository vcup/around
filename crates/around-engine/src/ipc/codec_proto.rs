//! Protobuf IPC codec — primary transport codec.
//!
//! Encodes/decodes IpcCommand and IpcResponse using Protocol Buffers.
//! Framing: varint-prefixed length, per spec §Data Model "ProtoCodec".

use prost::Message;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use super::codec::IpcCodec;
use super::proto;
use crate::ipc::types::{self, IpcResponse};
use crate::ipc::types::{ErrorCode, ResponseStatus, TrackState};

// ---------------------------------------------------------------------------
// Conversion helpers — map between proto-generated types and hand-written types
// ---------------------------------------------------------------------------

fn proto_command_to_ipc(c: proto::ipc_command::Command) -> std::io::Result<types::IpcCommand> {
  use proto::ipc_command::Command;
  match c {
    Command::Play(p) => Ok(types::IpcCommand::Play { path: p.path }),
    Command::Pause(_) => Ok(types::IpcCommand::Pause),
    Command::Resume(_) => Ok(types::IpcCommand::Resume),
    Command::Seek(s) => Ok(types::IpcCommand::Seek {
      position_ms: s.position_ms,
    }),
    Command::Stop(_) => Ok(types::IpcCommand::Stop),
    Command::Status(_) => Ok(types::IpcCommand::Status),
    Command::ListCodecs(_) => Ok(types::IpcCommand::ListCodecs),
    Command::LoadCodec(d) => Ok(types::IpcCommand::LoadCodec { path: d.path }),
    Command::LoadCodecBytes(d) => Ok(types::IpcCommand::LoadCodecBytes { data: d.data }),
    Command::Cleanup(_) => Ok(types::IpcCommand::Cleanup),
  }
}

fn track_state_to_proto(s: TrackState) -> i32 {
  match s {
    // proto TrackState enum values
    TrackState::Stopped => 0,
    TrackState::Playing => 1,
    TrackState::Paused => 2,
    TrackState::Buffering => 3,
    TrackState::Error => 4,
  }
}

fn codec_source_to_proto(s: &str) -> i32 {
  match s {
    "builtin" => 0,
    "extension" => 1,
    _ => 2, // dynamic or unknown
  }
}

fn error_code_to_proto(c: ErrorCode) -> String {
  match c {
    ErrorCode::FileNotFound => "FILE_NOT_FOUND".into(),
    ErrorCode::NoTrack => "NO_TRACK".into(),
    ErrorCode::NotSupported => "NOT_SUPPORTED".into(),
    ErrorCode::InvalidPosition => "INVALID_POSITION".into(),
    ErrorCode::CodecLoadFailed => "CODEC_LOAD_FAILED".into(),
  }
}

fn ipc_response_to_proto(resp: &IpcResponse) -> proto::IpcResponse {
  proto::IpcResponse {
    status: match resp.status {
      ResponseStatus::Ok => proto::Status::Ok,
      ResponseStatus::Error => proto::Status::Error,
    } as i32,
    state: resp.state.map(track_state_to_proto),
    position_ms: resp.position_ms,
    code: resp.code.map(error_code_to_proto),
    message: resp.message.clone(),
    track_id: resp.track_id,
    track: resp.track.as_ref().map(|t| proto::TrackInfo {
      id: t.id,
      path: t.path.clone(),
      format: t.format.clone(),
      duration_ms: t.duration_ms,
    }),
    codecs: resp
      .codecs
      .as_ref()
      .map(|ds| {
        ds.iter()
          .map(|d| proto::CodecDescriptor {
            name: d.name.clone(),
            formats: d.formats.clone(),
            source: codec_source_to_proto(&d.source),
            path: d.path.clone(),
          })
          .collect()
      })
      .unwrap_or_default(),
    device_lost: resp.device_lost,
    removed_files: resp.removed_files.clone().unwrap_or_default(),
  }
}

// ---------------------------------------------------------------------------
// Varint helpers
// ---------------------------------------------------------------------------

fn encode_varint(mut value: u64, buf: &mut Vec<u8>) {
  loop {
    let mut byte = (value & 0x7f) as u8;
    value >>= 7;
    if value != 0 {
      byte |= 0x80;
    }
    buf.push(byte);
    if value == 0 {
      break;
    }
  }
}

fn decode_varint(buf: &[u8]) -> Option<(u64, usize)> {
  let mut value: u64 = 0;
  let mut shift = 0;
  for (i, &byte) in buf.iter().enumerate() {
    value |= ((byte & 0x7f) as u64) << shift;
    if byte & 0x80 == 0 {
      return Some((value, i + 1));
    }
    shift += 7;
    if shift >= 64 {
      return None;
    }
  }
  None
}

// ---------------------------------------------------------------------------
// ProtoCodec
// ---------------------------------------------------------------------------

/// Protobuf-based IPC codec with varint-delimited framing.
#[derive(Clone)]
#[cfg(feature = "protobuf-ipc")]
pub(crate) struct ProtoCodec;

#[cfg(feature = "protobuf-ipc")]
impl IpcCodec for ProtoCodec {
  async fn read_command<R: AsyncRead + Unpin>(
    &self,
    reader: &mut BufReader<R>,
  ) -> std::io::Result<types::IpcCommand> {
    // Read varint length prefix byte-by-byte (varint is at most 10 bytes).
    let mut varint_buf = [0u8; 10];
    let mut varint_len = 0usize;
    loop {
      if varint_len >= 10 {
        return Err(std::io::Error::new(
          std::io::ErrorKind::InvalidData,
          "varint too long",
        ));
      }
      reader
        .read_exact(&mut varint_buf[varint_len..varint_len + 1])
        .await?;
      varint_len += 1;
      if varint_buf[varint_len - 1] & 0x80 == 0 {
        break;
      }
    }

    let (msg_len, _) = decode_varint(&varint_buf[..varint_len])
      .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid varint"))?;

    // Sanity cap: 64 MiB per message.
    if msg_len > 64 * 1024 * 1024 {
      return Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "message too large",
      ));
    }

    let mut msg_buf = vec![0u8; msg_len as usize];
    reader.read_exact(&mut msg_buf).await?;

    let proto_cmd = <proto::IpcCommand as Message>::decode(msg_buf.as_slice())
      .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    match proto_cmd.command {
      Some(c) => proto_command_to_ipc(c),
      None => Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "empty protobuf command",
      )),
    }
  }

  async fn write_response<W: AsyncWrite + Unpin>(
    &self,
    writer: &mut W,
    resp: &IpcResponse,
  ) -> std::io::Result<()> {
    let proto_resp = ipc_response_to_proto(resp);
    let mut msg_buf = Vec::with_capacity(proto_resp.encoded_len());
    proto_resp
      .encode(&mut msg_buf)
      .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let mut framed = Vec::with_capacity(10 + msg_buf.len());
    encode_varint(msg_buf.len() as u64, &mut framed);
    framed.extend_from_slice(&msg_buf);

    writer.write_all(&framed).await
  }
}
