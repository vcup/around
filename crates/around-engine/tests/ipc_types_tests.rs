//! Tests for IPC types (IpcCommand, IpcResponse, StreamStatus).
//! Platform-independent — run on all targets.

use around_engine::ipc::types::*;

#[test]
fn ipc_command_serialization_roundtrips() {
  #[expect(clippy::unwrap_used, reason = "hardcoded JSON literal is always valid")]
  let play_cmd: IpcCommand =
    serde_json::from_str(r#"{"command":"play","path":"/tmp/test.wav"}"#).unwrap();
  match play_cmd {
    IpcCommand::Play { path, stream_id } => {
      assert_eq!(path, "/tmp/test.wav");
      assert_eq!(stream_id, None);
    }
    _ => panic!("expected Play"),
  }

  #[expect(clippy::unwrap_used, reason = "hardcoded JSON literal is always valid")]
  let pause_cmd: IpcCommand = serde_json::from_str(r#"{"command":"pause"}"#).unwrap();
  assert!(matches!(pause_cmd, IpcCommand::Pause { .. }));

  #[expect(clippy::unwrap_used, reason = "hardcoded JSON literal is always valid")]
  let seek_cmd: IpcCommand =
    serde_json::from_str(r#"{"command":"seek","position_ms":30000}"#).unwrap();
  match seek_cmd {
    IpcCommand::Seek {
      position_ms,
      stream_id,
    } => {
      assert_eq!(position_ms, 30000);
      assert_eq!(stream_id, None);
    }
    _ => panic!("expected Seek"),
  }

  #[expect(clippy::unwrap_used, reason = "hardcoded JSON literal is always valid")]
  let status_cmd: IpcCommand = serde_json::from_str(r#"{"command":"status"}"#).unwrap();
  assert!(matches!(status_cmd, IpcCommand::Status { .. }));

  #[expect(clippy::unwrap_used, reason = "hardcoded JSON literal is always valid")]
  let stop_cmd: IpcCommand = serde_json::from_str(r#"{"command":"stop"}"#).unwrap();
  assert!(matches!(stop_cmd, IpcCommand::Stop { .. }));

  #[expect(clippy::unwrap_used, reason = "hardcoded JSON literal is always valid")]
  let list_cmd: IpcCommand = serde_json::from_str(r#"{"command":"list_codecs"}"#).unwrap();
  assert!(matches!(list_cmd, IpcCommand::ListCodecs));
}

#[expect(
  clippy::unwrap_used,
  reason = "IpcResponse::ok() always serializes successfully"
)]
#[test]
fn ipc_response_ok_format() {
  let resp = IpcResponse::ok();
  let json = serde_json::to_value(&resp).unwrap();
  assert_eq!(json["status"], "ok");
  // Default ok response should omit optional fields.
  assert!(json.get("state").is_none());
  assert!(json.get("stream_id").is_none());
}

#[expect(
  clippy::unwrap_used,
  reason = "IpcResponse::error() always serializes successfully — hardcoded input"
)]
#[test]
fn ipc_response_error_format() {
  let resp = IpcResponse::error(ErrorCode::NoTrack, "no active stream");
  let json = serde_json::to_value(&resp).unwrap();
  assert_eq!(json["status"], "error");
  assert_eq!(json["code"], "NO_TRACK");
  assert_eq!(json["message"], "no active stream");
}

#[expect(
  clippy::unwrap_used,
  reason = "json!() literal is valid by construction"
)]
#[test]
fn cleanup_command_serialization() {
  let cmd = serde_json::json!({"command": "cleanup"});
  let parsed: IpcCommand = serde_json::from_value(cmd).unwrap();
  match parsed {
    IpcCommand::Cleanup => {}
    _ => panic!("expected Cleanup"),
  }
}

#[test]
fn status_includes_device_lost() {
  // Verify IpcResponse::ok() omits device_lost when None (skip_serializing_if).
  let resp = IpcResponse::ok();
  #[expect(
    clippy::unwrap_used,
    reason = "IpcResponse::ok() always serializes successfully"
  )]
  let json = serde_json::to_value(&resp).unwrap();
  assert!(
    json.get("device_lost").is_none(),
    "IpcResponse::ok() should omit device_lost when None"
  );

  // Manually construct a status-style response with device_lost: false.
  let mut resp = IpcResponse::ok();
  resp.state = Some(PlaybackStatus::Stopped);
  resp.position_ms = Some(0);
  resp.device_lost = Some(false);
  #[expect(
    clippy::unwrap_used,
    reason = "hardcoded IpcResponse value always serializes successfully"
  )]
  let json = serde_json::to_value(&resp).unwrap();
  assert_eq!(json["status"], "ok");
  assert_eq!(json["state"], "stopped");
  assert_eq!(json["device_lost"], false);

  // When device_lost is Some(true), it should serialize.
  let mut resp = IpcResponse::ok();
  resp.device_lost = Some(true);
  #[expect(
    clippy::unwrap_used,
    reason = "hardcoded IpcResponse value always serializes successfully"
  )]
  let json = serde_json::to_value(&resp).unwrap();
  assert_eq!(json["device_lost"], true);
}

#[expect(clippy::unwrap_used, reason = "hardcoded JSON literal is always valid")]
#[test]
fn ipc_command_extra_fields_ignored() {
  // Extra fields must be ignored per forward-compatibility.
  let cmd: IpcCommand = serde_json::from_str(r#"{"command":"status","extra":"ignored"}"#).unwrap();
  assert!(matches!(cmd, IpcCommand::Status { .. }));
}

#[test]
fn ipc_command_missing_command_field_errors() {
  let result: Result<IpcCommand, _> = serde_json::from_str(r#"{}"#);
  assert!(result.is_err(), "missing command field must error");
}

#[test]
fn ipc_response_all_fields_populated() {
  let mut resp = IpcResponse::ok();
  resp.state = Some(PlaybackStatus::Playing);
  resp.position_ms = Some(12345);
  resp.track_id = Some(1);
  resp.track = Some(TrackInfo {
    id: 1,
    path: "/tmp/test.wav".into(),
    format: "wav".into(),
    duration_ms: 30000,
  });
  resp.codecs = Some(vec![CodecDescriptor {
    name: "test".into(),
    formats: vec!["wav".into()],
    source: "extension".into(),
    path: None,
  }]);
  resp.removed_files = Some(vec!["/tmp/old.so".into()]);
  resp.device_lost = Some(false);
  resp.stream_id = Some(42);

  #[expect(
    clippy::unwrap_used,
    reason = "hardcoded IpcResponse value always serializes successfully"
  )]
  let json = serde_json::to_value(&resp).unwrap();
  assert_eq!(json["status"], "ok");
  assert_eq!(json["state"], "playing");
  assert_eq!(json["position_ms"], 12345);
  assert_eq!(json["track_id"], 1);
  assert_eq!(json["stream_id"], 42);
  assert_eq!(json["track"]["format"], "wav");
  #[expect(
    clippy::unwrap_used,
    reason = "codecs field was just set to Some(vec![...]), so it exists and is an array"
  )]
  let codecs = json["codecs"].as_array().unwrap();
  assert_eq!(codecs.len(), 1);
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

#[expect(
  clippy::unwrap_used,
  reason = "IpcResponse::ok() with Vec<String> always serializes successfully"
)]
#[test]
fn ipc_response_removed_files_serialization() {
  let mut resp = IpcResponse::ok();
  resp.removed_files = Some(vec!["/tmp/old.so".into()]);
  let json = serde_json::to_value(&resp).unwrap();
  assert_eq!(json["removed_files"][0], "/tmp/old.so");
}

#[expect(
  clippy::unwrap_used,
  reason = "IpcResponse::ok() always serializes successfully"
)]
#[test]
fn ipc_response_removed_files_none_is_omitted() {
  let resp = IpcResponse::ok();
  let json = serde_json::to_value(&resp).unwrap();
  assert!(
    json.get("removed_files").is_none(),
    "removed_files should be omitted when None"
  );
}

#[test]
fn ipc_command_unknown_command_deserializes_to_default() {
  let result: Result<IpcCommand, _> = serde_json::from_str(r#"{"command":"unknown"}"#);
  assert!(result.is_err(), "unknown command should fail");
}

#[test]
fn ipc_command_stream_id_roundtrip() {
  #[expect(clippy::unwrap_used, reason = "hardcoded JSON literal is always valid")]
  let play_cmd: IpcCommand =
    serde_json::from_str(r#"{"command":"play","path":"/tmp/test.wav","stream_id":7}"#).unwrap();
  match play_cmd {
    IpcCommand::Play { stream_id, .. } => assert_eq!(stream_id, Some(7)),
    _ => panic!("expected Play"),
  }

  #[expect(clippy::unwrap_used, reason = "hardcoded JSON literal is always valid")]
  let pause_cmd: IpcCommand = serde_json::from_str(r#"{"command":"pause","stream_id":3}"#).unwrap();
  match pause_cmd {
    IpcCommand::Pause { stream_id } => assert_eq!(stream_id, Some(3)),
    _ => panic!("expected Pause"),
  }

  #[expect(clippy::unwrap_used, reason = "hardcoded JSON literal is always valid")]
  let status_cmd: IpcCommand = serde_json::from_str(r#"{"command":"status"}"#).unwrap();
  match status_cmd {
    IpcCommand::Status { stream_id } => assert_eq!(stream_id, None),
    _ => panic!("expected Status"),
  }
}

#[test]
fn stream_status_serialization() {
  let status = StreamStatus {
    stream_id: 1,
    status: PlaybackStatus::Playing,
    position_ms: 5000,
    seekable: true,
    device_lost: false,
    track: Some(TrackInfo {
      id: 1,
      path: "/tmp/test.wav".into(),
      format: "wav".into(),
      duration_ms: 30000,
    }),
  };
  #[expect(
    clippy::unwrap_used,
    reason = "hardcoded StreamStatus always serializes successfully"
  )]
  let json = serde_json::to_value(&status).unwrap();
  assert_eq!(json["stream_id"], 1);
  assert_eq!(json["status"], "playing");
  assert_eq!(json["position_ms"], 5000);
  assert_eq!(json["seekable"], true);
}

#[expect(
  clippy::unwrap_used,
  reason = "json!() literal is valid by construction"
)]
#[test]
fn play_with_stream_id() {
  let cmd = serde_json::json!({"command": "play", "path": "/tmp/test.wav", "stream_id": 5});
  let parsed: IpcCommand = serde_json::from_value(cmd).unwrap();
  match parsed {
    IpcCommand::Play { path, stream_id } => {
      assert_eq!(path, "/tmp/test.wav");
      assert_eq!(stream_id, Some(5));
    }
    _ => panic!("expected Play"),
  }
}

#[test]
fn response_with_streams_field() {
  let mut resp = IpcResponse::ok();
  resp.state = Some(PlaybackStatus::Playing);
  resp.stream_id = Some(1);
  resp.streams = Some(vec![StreamStatus {
    stream_id: 1,
    status: PlaybackStatus::Playing,
    position_ms: 5000,
    seekable: true,
    device_lost: false,
    track: None,
  }]);

  #[expect(
    clippy::unwrap_used,
    reason = "hardcoded IpcResponse value always serializes successfully"
  )]
  let json = serde_json::to_value(&resp).unwrap();
  assert_eq!(json["status"], "ok");
  assert_eq!(json["stream_id"], 1);
  #[expect(
    clippy::unwrap_used,
    reason = "streams field was just set to Some(vec![...]), so it exists and is an array"
  )]
  let streams = json["streams"].as_array().unwrap();
  assert_eq!(streams.len(), 1);
  assert_eq!(json["streams"][0]["stream_id"], 1);
  assert_eq!(json["streams"][0]["status"], "playing");
}
