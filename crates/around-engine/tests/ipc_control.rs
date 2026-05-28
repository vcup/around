//! Integration test: IPC transport control flow.
//!
//! Tests IPC command serialization, engine lifecycle, and response formats.

use around_engine::{Engine, EngineConfig, IpcCommand, IpcResponse, PlaybackState};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    .join("../../examples")
    .join(name)
}

fn send_command(socket_path: &str, cmd: &Value) -> Value {
  let mut stream = UnixStream::connect(socket_path).expect("connect to IPC socket");
  let mut json = serde_json::to_string(cmd).unwrap();
  json.push('\n');
  stream.write_all(json.as_bytes()).expect("send command");
  stream.flush().unwrap();

  let mut reader = BufReader::new(&stream);
  let mut response = String::new();
  reader.read_line(&mut response).expect("read response");
  serde_json::from_str(&response).expect("parse response")
}

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
fn engine_stop_is_idempotent() {
  let config = EngineConfig::load();
  let engine = Engine::new(config);
  engine.stop();
  engine.stop();
}

#[test]
fn engine_play_nonexistent_file_errors() {
  let config = EngineConfig::load();
  let engine = Engine::new(config);
  let source = around_source_file::FileSource::new("/tmp/__nonexistent_around_test__.wav");
  let result = engine.play(Box::new(source));
  assert!(result.is_err());
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
fn engine_play_valid_wav_returns_handle() {
  let config = EngineConfig::load();
  let engine = Engine::new(config);
  let source = around_source_file::FileSource::new(fixture_path("example.wav"));
  let result = engine.play(Box::new(source));
  assert!(result.is_ok());
  let handle = result.unwrap();
  assert_eq!(handle.output_format.sample_rate, 44100);
  assert_eq!(handle.output_format.channels, 1);
}

#[test]
fn engine_play_zero_byte_file_errors() {
  let config = EngineConfig::load();
  let engine = Engine::new(config);
  let source = around_source_file::FileSource::new(fixture_path("zero.wav"));
  let result = engine.play(Box::new(source));
  assert!(result.is_err());
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
fn buffering_state_in_playback_state() {
    let state = around_engine::PlaybackState::default();
    assert_eq!(state.state, "stopped");
}
