//! IPC types: shared between pipeline, IPC server, and CLI. Always compiled.

use around_core::SampleSpec;
use serde::{Deserialize, Serialize};

/// Playback lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackState {
  Stopped,
  Playing,
  Paused,
  Buffering,
  Error,
}

/// Response status — binary outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
  Ok,
  Error,
}

/// Error codes returned by IPC handlers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
  FileNotFound,
  NoTrack,
  NotSupported,
  InvalidPosition,
  CodecLoadFailed,
}

impl std::fmt::Display for TrackState {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      TrackState::Stopped => write!(f, "stopped"),
      TrackState::Playing => write!(f, "playing"),
      TrackState::Paused => write!(f, "paused"),
      TrackState::Buffering => write!(f, "buffering"),
      TrackState::Error => write!(f, "error"),
    }
  }
}

/// Shared playback state, updated by command handlers and readable via `status`.
// Manual Default: state defaults to "stopped"
impl Default for PlaybackState {
  fn default() -> Self {
    Self {
      playing: false,
      position_ms: 0,
      duration_ms: None,
      track_path: None,
      format_name: None,
      output_format: None,
      seekable: false,
      device_lost: false,
      epoch: 0,
      state: TrackState::Stopped,
    }
  }
}

#[derive(Debug, Clone)]
pub struct PlaybackState {
  pub playing: bool,
  pub position_ms: u64,
  pub duration_ms: Option<u64>,
  pub track_path: Option<String>,
  pub format_name: Option<String>,
  pub output_format: Option<SampleSpec>,
  pub seekable: bool,
  pub device_lost: bool,
  pub epoch: u64,
  pub state: TrackState,
}

/// Incoming IPC command, tagged by the `command` field in JSON.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum IpcCommand {
  Play {
    path: String,
  },
  Pause,
  Resume,
  Seek {
    position_ms: u64,
  },
  Stop,
  Status,
  ListCodecs,
  LoadCodec {
    path: String,
  },
  LoadCodecBytes {
    #[serde(deserialize_with = "deser_base64_bytes")]
    data: Vec<u8>,
  },
  Cleanup,
  /// Shut down the engine process.
  Shutdown,
}

/// Track metadata — codec-agnostic struct (replaces serde_json::Value).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackInfo {
  pub id: u64,
  pub path: String,
  pub format: String,
  pub duration_ms: u64,
}

/// Codec descriptor — serializable metadata for IPC responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodecDescriptor {
  pub name: String,
  pub formats: Vec<String>,
  pub source: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub path: Option<String>,
}

/// Deserialize a base64 string into `Vec<u8>`.
fn deser_base64_bytes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
  let s = String::deserialize(d)?;
  base64_decode(&s).map_err(serde::de::Error::custom)
}

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
  let input = input.trim_end_matches('=');
  let mut out = Vec::with_capacity(input.len() * 3 / 4);
  let mut buf: u32 = 0;
  let mut bits = 0u8;
  for c in input.chars() {
    let val = match c {
      'A'..='Z' => c as u8 - b'A',
      'a'..='z' => c as u8 - b'a' + 26,
      '0'..='9' => c as u8 - b'0' + 52,
      '+' => 62,
      '/' => 63,
      _ => return Err(format!("invalid base64 character: {}", c)),
    } as u32;
    buf = (buf << 6) | val;
    bits += 6;
    if bits >= 8 {
      bits -= 8;
      out.push((buf >> bits) as u8);
      buf &= (1 << bits) - 1;
    }
  }
  Ok(out)
}

/// Outgoing IPC response; all fields optional except `status`.
#[derive(Debug, Serialize, Deserialize)]
pub struct IpcResponse {
  pub status: ResponseStatus,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub state: Option<TrackState>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub position_ms: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub code: Option<ErrorCode>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub message: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub track_id: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub track: Option<TrackInfo>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub codecs: Option<Vec<CodecDescriptor>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub device_lost: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub removed_files: Option<Vec<String>>,
}

impl IpcResponse {
  pub fn ok() -> Self {
    Self {
      status: ResponseStatus::Ok,
      state: None,
      position_ms: None,
      code: None,
      message: None,
      track_id: None,
      track: None,
      codecs: None,
      removed_files: None,
      device_lost: None,
    }
  }

  pub fn error(code: ErrorCode, msg: &str) -> Self {
    Self {
      status: ResponseStatus::Error,
      state: None,
      position_ms: None,
      code: Some(code),
      message: Some(msg.into()),
      track_id: None,
      track: None,
      codecs: None,
      removed_files: None,
      device_lost: None,
    }
  }
}
