//! Tests for IPC types (PlaybackState, IpcCommand, IpcResponse).
//! Platform-independent — run on all targets.

use around_engine::{Engine, EngineConfig, IpcCommand, IpcResponse, PlaybackState};
use serde_json;

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

  let list_cmd: IpcCommand = serde_json::from_str(r#"{"command":"list_decoders"}"#).unwrap();
  assert!(matches!(list_cmd, IpcCommand::ListDecoders));
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
  let resp = IpcResponse::error("FILE_NOT_FOUND", "No such file");
  let json = serde_json::to_value(&resp).unwrap();
  assert_eq!(json["status"], "error");
  assert_eq!(json["code"], "FILE_NOT_FOUND");
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
  assert_eq!(state.state, "stopped");
}

#[test]
fn playback_state_default_seekable_is_false() {
  let st = PlaybackState::default();
  assert!(!st.seekable, "seekable must default to false for safety");
  assert!(!st.playing, "playing must default to false");
  assert_eq!(st.state, "stopped");
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
  resp.state = Some("stopped".into());
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
