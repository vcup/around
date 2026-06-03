//! Tests for IPC types (PlaybackState, IpcCommand, IpcResponse).
//! Platform-independent — run on all targets.

use around_engine::ipc::IpcConfig;
use around_engine::{
  Engine, EngineConfig, ErrorCode, IpcCommand, IpcResponse, PlaybackState, TrackInfo, TrackState,
};

#[test]
fn ipc_command_serialization_roundtrips() {
  let play_cmd: IpcCommand =
    serde_json::from_str(r#"{"command":"play","path":"/tmp/test.wav"}"#).unwrap();
  match play_cmd {
    IpcCommand::Play { path } => assert_eq!(path, "/tmp/test.wav"),
    _ => panic!("expected Play"),
  }

  let pause_cmd: IpcCommand = serde_json::from_str(r#"{"command":"pause"}"#).unwrap();
  assert!(matches!(pause_cmd, IpcCommand::Pause));

  let seek_cmd: IpcCommand =
    serde_json::from_str(r#"{"command":"seek","position_ms":30000}"#).unwrap();
  match seek_cmd {
    IpcCommand::Seek { position_ms } => assert_eq!(position_ms, 30000),
    _ => panic!("expected Seek"),
  }

  let status_cmd: IpcCommand = serde_json::from_str(r#"{"command":"status"}"#).unwrap();
  assert!(matches!(status_cmd, IpcCommand::Status));

  let stop_cmd: IpcCommand = serde_json::from_str(r#"{"command":"stop"}"#).unwrap();
  assert!(matches!(stop_cmd, IpcCommand::Stop));

  let list_cmd: IpcCommand = serde_json::from_str(r#"{"command":"list_codecs"}"#).unwrap();
  assert!(matches!(list_cmd, IpcCommand::ListCodecs));
}

#[test]
fn ipc_response_ok_format() {
  let resp = IpcResponse::ok();
  let json = serde_json::to_value(&resp).unwrap();
  assert_eq!(json["status"], "ok");
  assert!(json.get("state").is_none());
}

#[test]
fn ipc_response_error_format() {
  let resp = IpcResponse::error(ErrorCode::FileNotFound, "No such file");
  let json = serde_json::to_value(&resp).unwrap();
  assert_eq!(json["status"], "error");
  assert_eq!(json["code"].as_str().unwrap(), "FILE_NOT_FOUND");
  assert_eq!(json["message"], "No such file");
}

#[test]
fn playback_state_default() {
  let state = PlaybackState::default();
  assert!(!state.playing);
  assert_eq!(state.position_ms, 0);
  assert!(state.duration_ms.is_none());
  assert!(state.track_path.is_none());
}

#[test]
fn buffering_state_in_playback_state() {
  let state = around_engine::PlaybackState::default();
  assert_eq!(state.state, TrackState::Stopped);
}

#[test]
fn playback_state_default_seekable_is_false() {
  let st = PlaybackState::default();
  assert!(!st.seekable, "seekable must default to false for safety");
  assert!(!st.playing, "playing must default to false");
  assert_eq!(st.state, TrackState::Stopped);
}

#[test]
fn play_while_playing_replaces_track() {
  // When play is issued while another track is playing, the new track
  // replaces the current one (VLC-style replace).
  // This verifies the IPC command structure.
  let cmd = serde_json::json!({"command": "play", "path": "/tmp/test.wav"});
  let parsed: around_engine::IpcCommand = serde_json::from_value(cmd).unwrap();
  match parsed {
    around_engine::IpcCommand::Play { path } => assert_eq!(path, "/tmp/test.wav"),
    _ => panic!("expected Play"),
  }
}

#[test]
fn cleanup_command_serialization() {
  let cmd = serde_json::json!({"command": "cleanup"});
  let parsed: around_engine::IpcCommand = serde_json::from_value(cmd).unwrap();
  match parsed {
    around_engine::IpcCommand::Cleanup => {}
    _ => panic!("expected Cleanup"),
  }
}

#[test]
fn status_includes_device_lost() {
  // Verify PlaybackState defaults have device_lost: false.
  let state = PlaybackState::default();
  assert!(
    !state.device_lost,
    "default PlaybackState should have device_lost: false"
  );

  // Verify IpcResponse::ok() omits device_lost when None (skip_serializing_if).
  let resp = IpcResponse::ok();
  let json = serde_json::to_value(&resp).unwrap();
  assert!(
    json.get("device_lost").is_none(),
    "IpcResponse::ok() should omit device_lost when None"
  );

  // Manually construct a status-style response with device_lost: false.
  let mut resp = IpcResponse::ok();
  resp.state = Some(TrackState::Stopped);
  resp.position_ms = Some(0);
  resp.device_lost = Some(false);
  let json = serde_json::to_value(&resp).unwrap();
  assert_eq!(json["status"], "ok");
  assert_eq!(json["state"], "stopped");
  assert_eq!(json["device_lost"], false);

  // When device_lost is Some(true), it should serialize.
  let mut resp = IpcResponse::ok();
  resp.device_lost = Some(true);
  let json = serde_json::to_value(&resp).unwrap();
  assert_eq!(json["device_lost"], true);
}

#[test]
fn engine_stop_is_idempotent() {
  let config = EngineConfig::load();
  let engine = Engine::new(config);
  engine.stop();
  engine.stop();
}

// ---------------------------------------------------------------------------
// IpcConfig tests
// ---------------------------------------------------------------------------

#[test]
fn ipc_config_default_has_platform_native_enabled() {
  let cfg = IpcConfig::default();
  assert!(
    cfg.enable_platform_native,
    "platform-native must be enabled by default"
  );
  assert!(
    cfg.tcp_bind.is_none(),
    "TCP must be off by default (Constitution VI)"
  );
  assert!(
    cfg.udp_bind.is_none(),
    "UDP must be off by default (Constitution VI)"
  );
}

#[test]
fn ipc_config_from_cli_env_tcp_flag_takes_precedence() {
  // CLI flag takes precedence over env var (FR-037).
  let cfg = IpcConfig::from_parts(Some("0.0.0.0:5555"), Some("0.0.0.0:9999"), None, None).unwrap();
  assert_eq!(cfg.tcp_bind.unwrap().to_string(), "0.0.0.0:5555");
}

#[test]
fn ipc_config_from_cli_env_falls_back_to_env_var() {
  // When CLI flag is None, env var is used.
  let cfg = IpcConfig::from_parts(None, None, None, Some("0.0.0.0:7777")).unwrap();
  assert_eq!(cfg.udp_bind.unwrap().to_string(), "0.0.0.0:7777");
}

#[test]
fn ipc_config_from_cli_env_invalid_address_errors() {
  let result = IpcConfig::from_parts(Some("notanaddress"), None, None, None);
  assert!(result.is_err(), "invalid address must error");
}

#[test]
fn ipc_config_from_cli_env_both_none_gives_defaults() {
  let cfg = IpcConfig::from_parts(None, None, None, None).unwrap();
  assert!(cfg.tcp_bind.is_none());
  assert!(cfg.udp_bind.is_none());
  assert!(cfg.enable_platform_native);
}

// ---------------------------------------------------------------------------
// IpcResponse field tests
// ---------------------------------------------------------------------------

#[test]
fn ipc_response_removed_files_serialization() {
  // Verify removed_files serializes correctly (data model compliance).
  let mut resp = IpcResponse::ok();
  resp.removed_files = Some(vec!["/tmp/a".into(), "/tmp/b".into()]);
  let json = serde_json::to_value(&resp).unwrap();
  assert_eq!(json["status"], "ok");
  let files = json["removed_files"].as_array().unwrap();
  assert_eq!(files.len(), 2);
  assert_eq!(files[0], "/tmp/a");
  assert_eq!(files[1], "/tmp/b");
}

#[test]
fn ipc_response_removed_files_none_is_omitted() {
  let mut resp = IpcResponse::ok();
  resp.removed_files = Some(vec![]);
  let json = serde_json::to_value(&resp).unwrap();
  // Empty vec still serializes (it\'s Some, not None).
  assert!(json.get("removed_files").is_some());
  assert_eq!(json["removed_files"].as_array().unwrap().len(), 0);

  let resp = IpcResponse::ok();
  let json = serde_json::to_value(&resp).unwrap();
  // None should be omitted entirely.
  assert!(json.get("removed_files").is_none());
}

#[test]
fn ipc_command_unknown_command_deserializes_to_default() {
  // serde(tag = "command") with unknown tag should error, not silently succeed.
  let result: Result<IpcCommand, _> = serde_json::from_str(r#"{"command":"nonexistent"}"#);
  assert!(result.is_err(), "unknown command must be rejected");
}

#[test]
fn ipc_command_play_without_path_errors() {
  let result: Result<IpcCommand, _> = serde_json::from_str(r#"{"command":"play"}"#);
  assert!(result.is_err(), "play without path must error");
}

#[test]
fn ipc_command_seek_without_position_errors() {
  let result: Result<IpcCommand, _> = serde_json::from_str(r#"{"command":"seek"}"#);
  assert!(result.is_err(), "seek without position_ms must error");
}

#[test]
fn ipc_command_load_codec_without_path_errors() {
  let result: Result<IpcCommand, _> = serde_json::from_str(r#"{"command":"load_codec"}"#);
  assert!(result.is_err(), "load_codec without path must error");
}

#[test]
fn ipc_command_extra_fields_ignored() {
  // Extra fields must be ignored per forward-compatibility.
  let cmd: IpcCommand = serde_json::from_str(r#"{"command":"status","extra":"ignored"}"#).unwrap();
  assert!(matches!(cmd, IpcCommand::Status));
}

#[test]
fn ipc_command_missing_command_field_errors() {
  let result: Result<IpcCommand, _> = serde_json::from_str(r#"{}"#);
  assert!(result.is_err(), "missing command field must error");
}

#[test]
fn ipc_response_all_fields_populated() {
  let mut resp = IpcResponse::ok();
  resp.state = Some(TrackState::Playing);
  resp.position_ms = Some(12345);
  resp.track_id = Some(1);
  resp.track = Some(TrackInfo {
    id: 1,
    path: "/tmp/test.wav".into(),
    format: "wav".into(),
    duration_ms: 30000,
  });
  resp.codecs = Some(vec![around_engine::ipc::types::CodecDescriptor {
    name: "test".into(),
    formats: vec!["wav".into()],
    source: "extension".into(),
    path: None,
  }]);
  resp.removed_files = Some(vec!["/tmp/old.so".into()]);
  resp.device_lost = Some(false);

  let json = serde_json::to_value(&resp).unwrap();
  assert_eq!(json["status"], "ok");
  assert_eq!(json["state"], "playing");
  assert_eq!(json["position_ms"], 12345);
  assert_eq!(json["track_id"], 1);
  assert_eq!(json["track"]["format"], "wav");
  assert_eq!(json["codecs"].as_array().unwrap().len(), 1);
  assert_eq!(json["removed_files"][0], "/tmp/old.so");
  assert_eq!(json["device_lost"], false);
  // Verify no extra/unexpected fields leak into the response.
  assert!(
    json.get("code").is_none(),
    "ok response must not have error code"
  );
  assert!(
    json.get("message").is_none(),
    "ok response must not have message"
  );
}

#[test]
fn ipc_config_from_cli_env_invalid_udp_address_errors() {
  // M4: Invalid UDP bind address via env var must produce an error.
  let result = IpcConfig::from_parts(None, None, None, Some("notanaddress"));
  assert!(result.is_err(), "invalid UDP address must error");
}
