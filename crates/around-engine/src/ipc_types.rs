//! IPC types: shared between pipeline, IPC server, and CLI. Always compiled.

use around_core::SampleSpec;
use serde::{Deserialize, Serialize};

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
      state: "stopped".into(),
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
  pub state: String,
}

/// Incoming IPC command, tagged by the `command` field in JSON.
#[derive(Debug, Deserialize)]
#[serde(tag = "command")]
pub enum IpcCommand {
  #[serde(rename = "play")]
  Play { path: String },
  #[serde(rename = "pause")]
  Pause,
  #[serde(rename = "resume")]
  Resume,
  #[serde(rename = "seek")]
  Seek { position_ms: u64 },
  #[serde(rename = "stop")]
  Stop,
  #[serde(rename = "status")]
  Status,
  #[serde(rename = "list_decoders")]
  ListDecoders,
  #[serde(rename = "load_decoder")]
  LoadDecoder { path: String },
  #[serde(rename = "load_decoder_bytes")]
  LoadDecoderBytes { data: String, name: String },
  #[serde(rename = "cleanup")]
  Cleanup,
}

/// Outgoing IPC response; all fields optional except `status`.
#[derive(Debug, Serialize)]
pub struct IpcResponse {
  pub status: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub state: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub position_ms: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub code: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub message: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub track_id: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub track: Option<serde_json::Value>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub decoders: Option<Vec<serde_json::Value>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub device_lost: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub removed_files: Option<Vec<String>>,
}

impl IpcResponse {
  pub fn ok() -> Self {
    Self {
      status: "ok".into(),
      state: None,
      position_ms: None,
      code: None,
      message: None,
      track_id: None,
      track: None,
      decoders: None,
      removed_files: None,
      device_lost: None,
    }
  }

  pub fn error(code: &str, msg: &str) -> Self {
    Self {
      status: "error".into(),
      state: None,
      position_ms: None,
      code: Some(code.into()),
      message: Some(msg.into()),
      track_id: None,
      track: None,
      decoders: None,
      removed_files: None,
      device_lost: None,
    }
  }
}
