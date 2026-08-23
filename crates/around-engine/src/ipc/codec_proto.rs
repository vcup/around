//! Protobuf IPC codec — primary transport codec.
//!
//! Encodes/decodes IpcCommand and IpcResponse using Protocol Buffers.
//! Framing: varint-prefixed length, per spec §Data Model "ProtoCodec".

use prost::Message;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use super::codec::IpcCodec;
use super::proto;
use crate::ipc::types::{self, ErrorCode, IpcResponse, ResponseStatus};
// ---------------------------------------------------------------------------
// Conversion helpers — map between proto-generated types and hand-written types
// ---------------------------------------------------------------------------

fn proto_command_to_ipc(c: proto::ipc_command::Command) -> std::io::Result<types::IpcCommand> {
  use proto::ipc_command::Command;
  match c {
    Command::Play(p) => Ok(types::IpcCommand::Play {
      path: p.path,
      stream_id: p.stream_id,
    }),
    Command::Pause(p) => Ok(types::IpcCommand::Pause {
      stream_id: p.stream_id,
    }),
    Command::Resume(p) => Ok(types::IpcCommand::Resume {
      stream_id: p.stream_id,
    }),
    Command::Seek(s) => Ok(types::IpcCommand::Seek {
      position_ms: s.position_ms,
      stream_id: s.stream_id,
    }),
    Command::Stop(p) => Ok(types::IpcCommand::Stop {
      stream_id: p.stream_id,
    }),
    Command::Status(p) => Ok(types::IpcCommand::Status {
      stream_id: p.stream_id,
    }),
    Command::ListCodecs(_) => Ok(types::IpcCommand::ListCodecs),
    Command::LoadCodec(d) => Ok(types::IpcCommand::LoadCodec { path: d.path }),
    Command::LoadCodecBytes(d) => Ok(types::IpcCommand::LoadCodecBytes { data: d.data }),
    Command::Cleanup(_) => Ok(types::IpcCommand::Cleanup),
    Command::Shutdown(_) => Ok(types::IpcCommand::Shutdown),
  }
}

fn playback_status_to_proto(s: types::PlaybackStatus) -> i32 {
  match s {
    types::PlaybackStatus::Playing => 0,
    types::PlaybackStatus::Paused => 1,
    types::PlaybackStatus::Stopped => 2,
    types::PlaybackStatus::Buffering => 3,
    types::PlaybackStatus::Error => 4,
  }
}

#[expect(
  dead_code,
  reason = "protobuf IPC codec not fully wired; will be activated when protobuf-ipc feature is completed"
)]
fn proto_playback_status_from_i32(v: i32) -> std::io::Result<types::PlaybackStatus> {
  match v {
    0 => Ok(types::PlaybackStatus::Playing),
    1 => Ok(types::PlaybackStatus::Paused),
    2 => Ok(types::PlaybackStatus::Stopped),
    3 => Ok(types::PlaybackStatus::Buffering),
    4 => Ok(types::PlaybackStatus::Error),
    _ => Err(std::io::Error::new(
      std::io::ErrorKind::InvalidData,
      format!("unknown PlaybackStatus: {v}"),
    )),
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
fn sample_spec_to_proto(spec: &around_core::SampleSpec) -> proto::SampleSpec {
  proto::SampleSpec {
    sample_rate: spec.sample_rate,
    channels: u32::from(spec.channels),
    encoding: format!("{:?}", spec.encoding),
    interleave: format!("{:?}", spec.interleave),
    byte_order: format!("{:?}", spec.byte_order),
  }
}

fn ipc_response_to_proto(resp: &IpcResponse) -> proto::IpcResponse {
  proto::IpcResponse {
    status: match resp.status {
      ResponseStatus::Ok => proto::Status::Ok,
      ResponseStatus::Error => proto::Status::Error,
    } as i32,
    state: resp.state.map(playback_status_to_proto),
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
    output_spec: resp.output_spec.as_ref().map(sample_spec_to_proto),
    stream_id: resp.stream_id,
    seekable: resp.seekable,
    content_type: resp.content_type.clone(),
    streams: resp
      .streams
      .as_ref()
      .map(|ss| {
        ss.iter()
          .map(|s| proto::StreamStatus {
            stream_id: s.stream_id,
            status: playback_status_to_proto(s.status),
            position_ms: s.position_ms,
            seekable: s.seekable,
            device_lost: s.device_lost,
            output_spec: s.output_spec.as_ref().map(sample_spec_to_proto),
            track: s.track.as_ref().map(|t| proto::TrackInfo {
              id: t.id,
              path: t.path.clone(),
              format: t.format.clone(),
              duration_ms: t.duration_ms,
            }),
            content_type: s.content_type.clone(),
          })
          .collect()
      })
      .unwrap_or_default(),
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
    value |= u64::from(byte & 0x7f) << shift;
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

    let mut msg_buf = vec![
      0u8;
      usize::try_from(msg_len).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "message too large")
      })?
    ];
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

#[cfg(test)]
mod tests {
  #![expect(
    clippy::unwrap_used,
    reason = "the fixed test SampleSpec is valid by construction"
  )]
  use super::*;
  #[test]
  fn response_mapping_preserves_content_type_and_output_specs() {
    let spec =
      around_core::SampleSpec::interleaved(48_000, 2, around_core::PcmEncoding::F32).unwrap();
    let mut response = IpcResponse::ok();
    response.content_type = Some("audio/wav".into());
    response.output_spec = Some(spec);
    response.streams = Some(vec![types::StreamStatus {
      stream_id: 7,
      status: types::PlaybackStatus::Playing,
      position_ms: 12,
      seekable: true,
      device_lost: false,
      output_spec: Some(spec),
      track: None,
      content_type: Some("audio/flac".into()),
    }]);

    let mapped = ipc_response_to_proto(&response);

    assert_eq!(mapped.content_type.as_deref(), Some("audio/wav"));
    assert_eq!(
      mapped.output_spec.as_ref().map(|s| s.sample_rate),
      Some(48_000)
    );
    assert_eq!(mapped.output_spec.as_ref().map(|s| s.channels), Some(2));
    assert_eq!(mapped.streams.len(), 1);
    assert_eq!(
      mapped.streams[0].content_type.as_deref(),
      Some("audio/flac")
    );
    assert_eq!(
      mapped.streams[0]
        .output_spec
        .as_ref()
        .map(|s| s.encoding.as_str()),
      Some("F32")
    );
  }
}
