//! IPC types: shared between pipeline, IPC server, and CLI. Always compiled.

pub use around_core::PlaybackStatus;
use around_core::SampleSpec;
use serde::{Deserialize, Serialize};

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

/// Per-stream status snapshot for IPC responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamStatus {
  pub stream_id: u64,
  pub status: PlaybackStatus,
  pub position_ms: u64,
  pub seekable: bool,
  pub device_lost: bool,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub output_spec: Option<SampleSpec>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub track: Option<TrackInfo>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub content_type: Option<String>,
}

/// Incoming IPC command, tagged by the `command` field in JSON.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum IpcCommand {
  Play {
    path: String,
    #[serde(default)]
    stream_id: Option<u64>,
  },
  Pause {
    #[serde(default)]
    stream_id: Option<u64>,
  },
  Resume {
    #[serde(default)]
    stream_id: Option<u64>,
  },
  Seek {
    position_ms: u64,
    #[serde(default)]
    stream_id: Option<u64>,
  },
  Stop {
    #[serde(default)]
    stream_id: Option<u64>,
  },
  Status {
    #[serde(default)]
    stream_id: Option<u64>,
  },
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
  struct Base64Visitor;
  impl<'de> serde::de::Visitor<'de> for Base64Visitor {
    type Value = Vec<u8>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
      write!(f, "a base64-encoded string")
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Vec<u8>, E> {
      base64_decode(v).map_err(serde::de::Error::custom)
    }
  }
  d.deserialize_str(Base64Visitor)
}

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
  use base64::Engine;
  base64::engine::general_purpose::STANDARD
    .decode(input)
    .map_err(|e| e.to_string())
}

/// Outgoing IPC response; all fields optional except `status`.
#[derive(Debug, Serialize, Deserialize)]
pub struct IpcResponse {
  pub status: ResponseStatus,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub state: Option<PlaybackStatus>,
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
  #[serde(skip_serializing_if = "Option::is_none")]
  pub stream_id: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub content_type: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub seekable: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub output_spec: Option<SampleSpec>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub streams: Option<Vec<StreamStatus>>,
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
      device_lost: None,
      removed_files: None,
      stream_id: None,
      content_type: None,
      seekable: None,
      output_spec: None,
      streams: None,
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
      device_lost: None,
      removed_files: None,
      stream_id: None,
      content_type: None,
      seekable: None,
      output_spec: None,
      streams: None,
    }
  }
}
