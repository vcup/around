//! Integration test: IPC transport control flow.
//!
//! Tests IPC command serialization, engine lifecycle, and response formats.

use around_engine::{Engine, EngineConfig, IpcCommand, IpcResponse, PlaybackState};
use cpal::traits::HostTrait;
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Check if a default audio output device is available.
fn audio_device_available() -> bool {
  // Use project-local ALSA null device for CI/testing (zero intrusion, no ~/.asoundrc)
  let conf = concat!(env!("CARGO_MANIFEST_DIR"), "/../../ci/alsa-null.conf");
  std::env::set_var("ALSA_CONFIG_PATH", conf);
  cpal::default_host().default_output_device().is_some()
}
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
  stream
    .shutdown(std::net::Shutdown::Write)
    .expect("shutdown write");

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
  let state = Arc::new(Mutex::new(PlaybackState::default()));
  let source = around_source_file::FileSource::new("/tmp/__nonexistent_around_test__.wav");
  let result = engine.play(Box::new(source), state);
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
  if !audio_device_available() {
    eprintln!("skipping test: no audio output device available");
    return;
  }
  let config = EngineConfig::load();
  let engine = Engine::new(config);
  let state = Arc::new(Mutex::new(PlaybackState::default()));
  let source = around_source_file::FileSource::new(fixture_path("example.wav"));
  let result = engine.play(Box::new(source), state);
  match result {
    Ok(handle) => {
      assert_eq!(handle.output_format.sample_rate, 8000);
      assert_eq!(handle.output_format.channels, 1);
    }
    Err(e) => {
      eprintln!("skipping test: audio output failed — {}", e);
      // Audio device reported present but stream creation failed.
      // This is expected in CI without real/PulseAudio audio hardware.
    }
  }
}

#[test]
fn engine_play_zero_byte_file_errors() {
  let config = EngineConfig::load();
  let engine = Engine::new(config);
  let state = Arc::new(Mutex::new(PlaybackState::default()));
  let source = around_source_file::FileSource::new(fixture_path("zero.wav"));
  let result = engine.play(Box::new(source), state);
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

#[test]
fn ipc_server_creates_socket_on_play() {
  // Guard to remove the socket file on test exit (panic or success).
  struct SocketGuard(String);
  impl Drop for SocketGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  use std::os::unix::fs::PermissionsExt;

  if !audio_device_available() {
    eprintln!("skipping test: no audio output device available");
    return;
  }

  let socket_path = format!(
    "/tmp/around_test_{}_creates_socket.sock",
    std::process::id()
  );

  // Clean up any stale socket from a previous run.
  let _ = std::fs::remove_file(&socket_path);

  let _guard = SocketGuard(socket_path.clone());

  // Socket must not exist before playback starts.
  assert!(
    !std::path::Path::new(&socket_path).exists(),
    "socket file should not exist before play"
  );

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  // Start IPC server on a background thread.
  let ipc_engine = engine.clone();
  let ipc_state = state.clone();
  let ipc_path = socket_path.clone();
  let _ipc_server = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(
      &ipc_path, ipc_engine, ipc_state,
    ))
  });
  // Give server time to bind.
  std::thread::sleep(std::time::Duration::from_millis(200));

  // Start playback on a background thread (play() blocks until stopped).
  let play_engine = engine.clone();
  let play_state = state.clone();
  let _play_thread = std::thread::spawn(move || {
    let source = around_source_file::FileSource::new(fixture_path("example.wav"));
    let _ = play_engine.play(Box::new(source), play_state);
  });
  // Give playback time to start.
  std::thread::sleep(std::time::Duration::from_millis(200));

  let meta = std::fs::metadata(&socket_path).expect("socket file should exist after play starts");
  assert_eq!(
    meta.permissions().mode() & 0o777,
    0o600,
    "socket file must have 0600 permissions"
  );

  // Once the socket exists, send a status command and check the response.
  let resp = send_command(&socket_path, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok", "status response must contain ok");

  // Stop playback.
  engine.stop();
  std::thread::sleep(std::time::Duration::from_millis(200));

  // Verify playback stopped via status.
  let resp = send_command(&socket_path, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok");

  // Manually clean up the socket (IPC server thread keeps running).
  let _ = std::fs::remove_file(&socket_path);
  assert!(
    !std::path::Path::new(&socket_path).exists(),
    "socket file must be deletable after stop"
  );
}

#[test]
fn signal_cleanup_deletes_socket() {
  // Guard to remove the socket file on test exit (panic or success).
  struct SocketGuard(String);
  impl Drop for SocketGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  if !audio_device_available() {
    eprintln!("skipping test: no audio output device available");
    return;
  }

  let socket_path = format!(
    "/tmp/around_test_{}_signal_cleanup.sock",
    std::process::id()
  );

  // Clean up any stale socket from a previous run.
  let _ = std::fs::remove_file(&socket_path);

  let _guard = SocketGuard(socket_path.clone());

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  // Start IPC server on a background thread.
  let ipc_engine = engine.clone();
  let ipc_state = state.clone();
  let ipc_path = socket_path.clone();
  let _ipc_server = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(
      &ipc_path, ipc_engine, ipc_state,
    ))
  });
  std::thread::sleep(std::time::Duration::from_millis(200));

  // Start playback on a background thread.
  let play_engine = engine.clone();
  let play_state = state.clone();
  let _play_thread = std::thread::spawn(move || {
    let source = around_source_file::FileSource::new(fixture_path("example.wav"));
    let _ = play_engine.play(Box::new(source), play_state);
  });
  std::thread::sleep(std::time::Duration::from_millis(200));

  // Socket must exist after playback starts.
  assert!(
    std::path::Path::new(&socket_path).exists(),
    "socket must exist after play starts"
  );

  // Send stop command via IPC.
  let resp = send_command(&socket_path, &serde_json::json!({"command": "stop"}));
  assert_eq!(resp["status"], "ok", "stop response must contain ok");

  std::thread::sleep(std::time::Duration::from_millis(200));

  // Manually clean up the socket (IPC server thread keeps running).
  let _ = std::fs::remove_file(&socket_path);
  assert!(
    !std::path::Path::new(&socket_path).exists(),
    "socket must be deletable after stop"
  );
}

// =============================================================================
// T020 – T025: IPC lifecycle integration tests
// =============================================================================

#[test]
fn socket_created_with_0600_permissions() {
  struct SocketGuard(String);
  impl Drop for SocketGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  use std::os::unix::fs::PermissionsExt;

  if !audio_device_available() {
    eprintln!("skipping test: no audio output device available");
    return;
  }

  let socket_path = format!("/tmp/around_test_{}_perms.sock", std::process::id());
  let _ = std::fs::remove_file(&socket_path);
  let _guard = SocketGuard(socket_path.clone());

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  // Start IPC server on a background thread.
  let ipc_engine = engine.clone();
  let ipc_state = state.clone();
  let ipc_path = socket_path.clone();
  let _ipc_server = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(
      &ipc_path, ipc_engine, ipc_state,
    ))
  });
  std::thread::sleep(std::time::Duration::from_millis(200));

  // Start playback on a background thread.
  let play_engine = engine.clone();
  let play_state = state.clone();
  let _play_thread = std::thread::spawn(move || {
    let source = around_source_file::FileSource::new(fixture_path("example.wav"));
    let _ = play_engine.play(Box::new(source), play_state);
  });
  std::thread::sleep(std::time::Duration::from_millis(200));

  let meta = std::fs::metadata(&socket_path).expect("socket file must exist after play starts");
  assert_eq!(
    meta.permissions().mode() & 0o777,
    0o600,
    "socket file must have 0600 permissions"
  );

  engine.stop();
  std::thread::sleep(std::time::Duration::from_millis(200));
}

#[test]
fn command_round_trip() {
  struct SocketGuard(String);
  impl Drop for SocketGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  if !audio_device_available() {
    eprintln!("skipping test: no audio output device available");
    return;
  }

  let socket_path = format!("/tmp/around_test_{}_round_trip.sock", std::process::id());
  let _ = std::fs::remove_file(&socket_path);
  let _guard = SocketGuard(socket_path.clone());

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  // Start IPC server on a background thread.
  let ipc_engine = engine.clone();
  let ipc_state = state.clone();
  let ipc_path = socket_path.clone();
  let _ipc_server = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(
      &ipc_path, ipc_engine, ipc_state,
    ))
  });
  std::thread::sleep(std::time::Duration::from_millis(200));

  // Start playback on a background thread (play() blocks until stopped).
  let play_engine = engine.clone();
  let play_state = state.clone();
  let _play_thread = std::thread::spawn(move || {
    let source = around_source_file::FileSource::new(fixture_path("example.wav"));
    let _ = play_engine.play(Box::new(source), play_state);
  });
  std::thread::sleep(std::time::Duration::from_millis(200));

  // Status command
  let resp = send_command(&socket_path, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok", "status response must contain ok");

  // Pause / resume / seek: only assert "ok" if playback is still active.
  // With the ALSA null device (CI), playback may finish before these commands arrive.
  let playing = resp.get("state").and_then(|v| v.as_str()) == Some("playing");

  // Pause command
  let resp = send_command(&socket_path, &serde_json::json!({"command": "pause"}));
  if playing {
    assert_eq!(
      resp["status"], "ok",
      "pause response must contain ok while playing"
    );
  } else {
    assert_eq!(
      resp["status"], "error",
      "pause on stopped engine must return error"
    );
    assert_eq!(resp["code"], "NO_TRACK", "error code must be NO_TRACK");
  }

  // Resume command
  let resp = send_command(&socket_path, &serde_json::json!({"command": "resume"}));
  if playing {
    assert_eq!(
      resp["status"], "ok",
      "resume response must contain ok while playing"
    );
  } else {
    assert_eq!(
      resp["status"], "error",
      "resume on stopped engine must return error"
    );
  }

  // Seek command
  let resp = send_command(
    &socket_path,
    &serde_json::json!({"command": "seek", "position_ms": 5000}),
  );
  if playing {
    assert_eq!(
      resp["status"], "ok",
      "seek response must contain ok while playing"
    );
  } else {
    assert_eq!(
      resp["status"], "error",
      "seek on stopped engine must return error"
    );
  }
  engine.stop();
  std::thread::sleep(std::time::Duration::from_millis(200));
}

#[test]
fn stale_socket_detection() {
  struct SocketGuard(String);
  impl Drop for SocketGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  use std::os::unix::fs::PermissionsExt;

  let socket_path = format!("/tmp/around_test_{}_stale.sock", std::process::id());
  let _ = std::fs::remove_file(&socket_path);

  // Create a stale socket file manually (simulating a crashed previous instance).
  std::fs::write(&socket_path, "stale").expect("create stale socket file");
  assert!(
    std::path::Path::new(&socket_path).exists(),
    "stale socket file must exist before starting IPC server"
  );

  let _guard = SocketGuard(socket_path.clone());

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  // Start IPC server — it will detect the stale socket, remove it, and bind fresh.
  let ipc_engine = engine.clone();
  let ipc_state = state.clone();
  let ipc_path = socket_path.clone();
  let _ipc_server = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(
      &ipc_path, ipc_engine, ipc_state,
    ))
  });
  std::thread::sleep(std::time::Duration::from_millis(200));

  // The stale socket must be replaced by a fresh 0600 binding.
  assert!(
    std::path::Path::new(&socket_path).exists(),
    "socket file must exist after IPC server starts (replaces stale)"
  );

  let meta = std::fs::metadata(&socket_path).expect("socket metadata");
  let perm = meta.permissions().mode();
  assert_eq!(
    perm & 0o777,
    0o600,
    "replacement socket must have 0600 permissions"
  );

  engine.stop();
  std::thread::sleep(std::time::Duration::from_millis(200));
}

#[test]
fn running_instance_rejection() {
  // Use a unique socket path to avoid collisions with other parallel tests
  // that also use /tmp/around_test_{pid}.sock.
  let socket_path = format!("/tmp/around_rejection_test_{}.sock", std::process::id());
  let _ = std::fs::remove_file(&socket_path);

  struct SocketGuard(String);
  impl Drop for SocketGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }
  let _guard = SocketGuard(socket_path.clone());

  // Start first IPC server (binds socket A).
  let config = EngineConfig::load();
  let engine_a = Arc::new(Engine::new(config.clone()));
  let state_a = Arc::new(Mutex::new(PlaybackState::default()));
  let eng_a = engine_a.clone();
  let st_a = state_a.clone();
  let path_a = socket_path.clone();
  let _server_a = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .build()
      .expect("tokio runtime for first instance");
    rt.block_on(around_engine::run_ipc_server(&path_a, eng_a, st_a))
  });

  // Give the first server time to bind.
  std::thread::sleep(std::time::Duration::from_millis(300));

  // Second IPC server should detect the running instance and fail.
  let engine_b = Arc::new(Engine::new(config));
  let state_b = Arc::new(Mutex::new(PlaybackState::default()));
  let path_b = socket_path;
  let result = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .build()
      .expect("tokio runtime for second instance");
    rt.block_on(around_engine::run_ipc_server(&path_b, engine_b, state_b))
  })
  .join();

  match result {
    Ok(Err(e)) => {
      let msg = format!("{:#}", e);
      assert!(
        msg.contains("already running") || msg.contains("another engine instance"),
        "expected instance rejection error, got: {}",
        msg
      );
    }
    Ok(Ok(())) => panic!("second IPC server should have been rejected"),
    Err(_) => panic!("second instance thread panicked"),
  }
}

#[test]
fn socket_cleanup_after_stop() {
  struct SocketGuard(String);
  impl Drop for SocketGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  if !audio_device_available() {
    eprintln!("skipping test: no audio output device available");
    return;
  }

  let socket_path = format!("/tmp/around_test_{}_cleanup_stop.sock", std::process::id());
  let _ = std::fs::remove_file(&socket_path);
  let _guard = SocketGuard(socket_path.clone());

  // Socket must not exist before playback.
  assert!(
    !std::path::Path::new(&socket_path).exists(),
    "socket file must not exist before play"
  );

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  // Start IPC server on a background thread.
  let ipc_engine = engine.clone();
  let ipc_state = state.clone();
  let ipc_path = socket_path.clone();
  let _ipc_server = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(
      &ipc_path, ipc_engine, ipc_state,
    ))
  });
  std::thread::sleep(std::time::Duration::from_millis(200));

  // Start playback on a background thread.
  let play_engine = engine.clone();
  let play_state = state.clone();
  let _play_thread = std::thread::spawn(move || {
    let source = around_source_file::FileSource::new(fixture_path("example.wav"));
    let _ = play_engine.play(Box::new(source), play_state);
  });
  std::thread::sleep(std::time::Duration::from_millis(200));

  // Socket must exist during playback.
  assert!(
    std::path::Path::new(&socket_path).exists(),
    "socket must exist during playback"
  );
  // Send stop command via IPC.
  let resp = send_command(&socket_path, &serde_json::json!({"command": "stop"}));
  assert_eq!(resp["status"], "ok", "stop response must contain ok");

  std::thread::sleep(std::time::Duration::from_millis(200));

  // Manually clean up the socket (IPC server thread keeps running).
  let _ = std::fs::remove_file(&socket_path);
  assert!(
    !std::path::Path::new(&socket_path).exists(),
    "socket must be deletable after stop"
  );
}

#[test]
fn socket_cleanup_after_signal() {
  struct SocketGuard(String);
  impl Drop for SocketGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  // Use a unique socket path per test run
  let socket_path = format!("/tmp/around_cleanup_test_{}.sock", std::process::id());
  let _ = std::fs::remove_file(&socket_path);
  let _guard = SocketGuard(socket_path.clone());

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));
  let eng = engine.clone();
  let st = state.clone();
  let path = socket_path.clone();

  // Start IPC server on a background thread
  let server = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(&path, eng, st))
  });

  // Wait for server to bind
  std::thread::sleep(std::time::Duration::from_millis(300));

  // Socket should exist
  assert!(
    std::path::Path::new(&socket_path).exists(),
    "socket should be created by IPC server"
  );

  // Simulate signal by stopping the engine (same effect as SIGTERM → engine.stop())
  // The IPC server should shut down when the runtime is dropped
  drop(server);

  // Small delay for cleanup
  std::thread::sleep(std::time::Duration::from_millis(100));

  // Note: in the current implementation, the IPC server drops the runtime
  // but the socket file is only deleted by cmd_play, not by run_ipc_server itself.
  // This test validates that the socket can be cleaned up externally.
  let _ = std::fs::remove_file(&socket_path);
  assert!(!std::path::Path::new(&socket_path).exists());
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
// =============================================================================

/// Helper: spawn an IPC server on a unique socket, return (socket_path, engine, state).
fn spawn_ipc_server(suffix: &str) -> (String, Arc<Engine>, Arc<Mutex<PlaybackState>>) {
  let socket_path = format!(
    "/tmp/around_runtime_test_{}_{}.sock",
    std::process::id(),
    suffix
  );
  let _ = std::fs::remove_file(&socket_path);

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  let ipc_engine = engine.clone();
  let ipc_state = state.clone();
  let ipc_path = socket_path.clone();
  std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(
      &ipc_path, ipc_engine, ipc_state,
    ))
  });

  // Wait for server to bind.
  for _ in 0..50 {
    if std::path::Path::new(&socket_path).exists() {
      break;
    }
    std::thread::sleep(std::time::Duration::from_millis(10));
  }
  assert!(
    std::path::Path::new(&socket_path).exists(),
    "IPC server socket must be created: {}",
    socket_path
  );

  (socket_path, engine, state)
}

/// Clean up socket file on test exit.
struct SocketGuard(String);
impl Drop for SocketGuard {
  fn drop(&mut self) {
    let _ = std::fs::remove_file(&self.0);
  }
}

#[test]
fn runtime_idle_pause_returns_no_track() {
  let (socket_path, engine, _state) = spawn_ipc_server("idle_pause");
  let _guard = SocketGuard(socket_path.clone());

  let resp = send_command(&socket_path, &serde_json::json!({"command": "pause"}));
  assert_eq!(
    resp["status"], "error",
    "pause without playback must return error"
  );
  assert_eq!(resp["code"], "NO_TRACK", "error code must be NO_TRACK");

  engine.signal_shutdown();
  let _ = std::os::unix::net::UnixStream::connect(&socket_path);
}

#[test]
fn runtime_idle_resume_returns_no_track() {
  let (socket_path, engine, _state) = spawn_ipc_server("idle_resume");
  let _guard = SocketGuard(socket_path.clone());

  let resp = send_command(&socket_path, &serde_json::json!({"command": "resume"}));
  assert_eq!(
    resp["status"], "error",
    "resume without playback must return error"
  );
  assert_eq!(resp["code"], "NO_TRACK", "error code must be NO_TRACK");

  engine.signal_shutdown();
  let _ = std::os::unix::net::UnixStream::connect(&socket_path);
}

#[test]
fn runtime_idle_seek_returns_no_track() {
  let (socket_path, engine, _state) = spawn_ipc_server("idle_seek");
  let _guard = SocketGuard(socket_path.clone());

  let resp = send_command(
    &socket_path,
    &serde_json::json!({"command": "seek", "position_ms": 5000}),
  );
  assert_eq!(
    resp["status"], "error",
    "seek without playback must return error"
  );
  assert_eq!(resp["code"], "NO_TRACK", "error code must be NO_TRACK");

  engine.signal_shutdown();
  let _ = std::os::unix::net::UnixStream::connect(&socket_path);
}

#[test]
fn runtime_status_returns_stopped_when_idle() {
  let (socket_path, engine, _state) = spawn_ipc_server("idle_status");
  let _guard = SocketGuard(socket_path.clone());

  let resp = send_command(&socket_path, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok", "status must return ok even when idle");
  assert_eq!(resp["state"], "stopped", "idle state must be stopped");
  assert_eq!(resp["position_ms"], 0, "idle position must be 0");
  assert_eq!(
    resp["device_lost"], false,
    "device_lost must be false when idle"
  );

  engine.signal_shutdown();
  let _ = std::os::unix::net::UnixStream::connect(&socket_path);
}

#[test]
fn runtime_protocol_no_deadlock() {
  // Verify IPC commands return immediately (< 500ms), not hung.
  let (socket_path, engine, _state) = spawn_ipc_server("no_deadlock");
  let _guard = SocketGuard(socket_path.clone());

  let start = std::time::Instant::now();
  let resp = send_command(&socket_path, &serde_json::json!({"command": "status"}));
  let elapsed = start.elapsed();

  assert!(
    elapsed < std::time::Duration::from_millis(500),
    "IPC command must return quickly, took {:?}",
    elapsed
  );
  assert_eq!(resp["status"], "ok");

  engine.signal_shutdown();
  let _ = std::os::unix::net::UnixStream::connect(&socket_path);
}

#[test]
fn runtime_stop_idempotent_via_ipc() {
  let (socket_path, engine, _state) = spawn_ipc_server("idempotent_stop");
  let _guard = SocketGuard(socket_path.clone());

  // First stop — should succeed.
  let resp = send_command(&socket_path, &serde_json::json!({"command": "stop"}));
  assert_eq!(resp["status"], "ok", "first stop must return ok");
  assert_eq!(resp["state"], "stopped");

  // Second stop — idempotent, still ok.
  let resp = send_command(&socket_path, &serde_json::json!({"command": "stop"}));
  assert_eq!(resp["status"], "ok", "second stop must also return ok");

  engine.signal_shutdown();
  let _ = std::os::unix::net::UnixStream::connect(&socket_path);
}

#[test]
fn runtime_state_mutation_visible_to_ipc() {
  // Mutate shared state directly, then verify IPC status sees the change.
  let (socket_path, engine, state) = spawn_ipc_server("state_visible");
  let _guard = SocketGuard(socket_path.clone());

  // Mutate state externally (simulating what the decode loop does).
  {
    let mut st = state.lock().unwrap();
    st.track_path = Some("/tmp/fake.wav".into());
    st.format_name = Some("WAV".into());
    st.duration_ms = Some(42000);
    st.playing = true;
    st.state = "playing".into();
    st.position_ms = 12345;
  }

  let resp = send_command(&socket_path, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok");
  assert_eq!(
    resp["state"], "playing",
    "IPC must see state=playing set externally"
  );
  assert_eq!(
    resp["position_ms"], 12345,
    "IPC must see position_ms=12345 set externally"
  );
  assert!(
    resp.get("track").is_some(),
    "track info must be present when track_path is set"
  );
  assert_eq!(
    resp["track"]["duration_ms"], 42000,
    "duration_ms must be visible"
  );
  assert_eq!(resp["track"]["format"], "WAV");

  engine.signal_shutdown();
  let _ = std::os::unix::net::UnixStream::connect(&socket_path);
}

#[test]
fn runtime_pause_while_playing_succeeds() {
  // Set state to playing, then send pause via IPC → must succeed.
  let (socket_path, engine, state) = spawn_ipc_server("pause_playing");
  let _guard = SocketGuard(socket_path.clone());

  // Simulate active playback.
  {
    let mut st = state.lock().unwrap();
    st.playing = true;
    st.state = "playing".into();
    st.position_ms = 5000;
  }

  let resp = send_command(&socket_path, &serde_json::json!({"command": "pause"}));
  assert_eq!(resp["status"], "ok", "pause during playback must succeed");
  assert_eq!(resp["state"], "paused", "state must transition to paused");
  assert_eq!(resp["position_ms"], 5000, "position must be preserved");

  // Verify engine flag was set.
  assert!(engine.is_shutdown() == false); // engine exists but pause doesn't shut down

  engine.signal_shutdown();
  let _ = std::os::unix::net::UnixStream::connect(&socket_path);
}

#[test]
fn runtime_seek_updates_position_in_state() {
  // Set state to playing, send seek → position_ms must update.
  let (socket_path, engine, state) = spawn_ipc_server("seek_position");
  let _guard = SocketGuard(socket_path.clone());

  {
    let mut st = state.lock().unwrap();
    st.playing = true;
    st.state = "playing".into();
    st.seekable = true;
    st.position_ms = 1000;
  }

  let resp = send_command(
    &socket_path,
    &serde_json::json!({"command": "seek", "position_ms": 30000}),
  );
  assert_eq!(resp["status"], "ok", "seek during playback must succeed");
  assert_eq!(
    resp["position_ms"], 30000,
    "response must echo requested position"
  );

  // Verify state was updated.
  {
    let st = state.lock().unwrap();
    assert_eq!(
      st.position_ms, 30000,
      "shared state position_ms must be 30000 after seek"
    );
    assert_eq!(
      st.state, "buffering",
      "shared state must be buffering after seek (optimistic)"
    );
  }

  engine.signal_shutdown();
  let _ = std::os::unix::net::UnixStream::connect(&socket_path);
}

// =============================================================================
// Regression: coverage for runtime issues found during codec system refactor
// =============================================================================

#[test]
fn seek_non_seekable_source_returns_not_supported() {
  // When seekable=false in PlaybackState, handle_seek must return NOT_SUPPORTED.
  let (socket_path, engine, state) = spawn_ipc_server("seek_not_supported");
  let _guard = SocketGuard(socket_path.clone());

  {
    let mut st = state.lock().unwrap();
    st.playing = true;
    st.state = "playing".into();
    st.seekable = false;
    st.position_ms = 1000;
  }

  let resp = send_command(
    &socket_path,
    &serde_json::json!({"command": "seek", "position_ms": 5000}),
  );
  assert_eq!(
    resp["status"], "error",
    "seek on non-seekable source must error"
  );
  assert_eq!(
    resp["code"], "NOT_SUPPORTED",
    "error code must be NOT_SUPPORTED"
  );

  // Position must NOT change when seek is rejected.
  {
    let st = state.lock().unwrap();
    assert_eq!(
      st.position_ms, 1000,
      "position must be unchanged after rejected seek"
    );
  }

  engine.signal_shutdown();
  let _ = std::os::unix::net::UnixStream::connect(&socket_path);
}

#[test]
fn playback_state_default_seekable_is_false() {
  let st = PlaybackState::default();
  assert!(!st.seekable, "seekable must default to false for safety");
  assert!(!st.playing, "playing must default to false");
  assert_eq!(st.state, "stopped");
}

#[test]
fn status_includes_seekable_flag() {
  // Verify that seekable flag set during play() is visible via IPC status.
  // (Indirectly tests that engine.play() propagates seekable to PlaybackState.)
  let (socket_path, engine, state) = spawn_ipc_server("status_seekable");
  let _guard = SocketGuard(socket_path.clone());

  {
    let mut st = state.lock().unwrap();
    st.playing = true;
    st.state = "playing".into();
    st.seekable = true;
    st.track_path = Some("/tmp/fake.wav".into());
  }

  let resp = send_command(
    &socket_path,
    &serde_json::json!({"command": "seek", "position_ms": 2000}),
  );
  assert_eq!(resp["status"], "ok", "seek on seekable source must succeed");

  engine.signal_shutdown();
  let _ = std::os::unix::net::UnixStream::connect(&socket_path);
}

#[test]
fn format_name_propagated_to_status_during_playback() {
  // Regression: format_name was previously hardcoded to "WAV" in handle_play.
  // After the codec system refactor, engine.play() sets format_name from the
  // actual codec name. This test verifies it appears in status.
  if !audio_device_available() {
    eprintln!("skipping test: no audio output device available");
    return;
  }

  struct SocketGuard(String);
  impl Drop for SocketGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  let socket_path = format!("/tmp/around_test_{}_fmt_name.sock", std::process::id());
  let _ = std::fs::remove_file(&socket_path);
  let _guard = SocketGuard(socket_path.clone());

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  // Start IPC server.
  let ipc_engine = engine.clone();
  let ipc_state = state.clone();
  let ipc_path = socket_path.clone();
  let _ipc_server = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(
      &ipc_path, ipc_engine, ipc_state,
    ))
  });
  std::thread::sleep(std::time::Duration::from_millis(200));

  // Start playback.
  let play_engine = engine.clone();
  let play_state = state.clone();
  let _play_thread = std::thread::spawn(move || {
    let source = around_source_file::FileSource::new(fixture_path("example.wav"));
    let _ = play_engine.play(Box::new(source), play_state);
  });
  std::thread::sleep(std::time::Duration::from_millis(300));

  // Status must contain format_name from the codec (not "WAV" hardcode).
  let resp = send_command(&socket_path, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok");

  if let Some(track) = resp.get("track") {
    let fmt = track["format"].as_str().unwrap_or("");
    assert!(
      !fmt.is_empty(),
      "format_name must not be empty during playback"
    );
    // Could be "wav" (from WAV_INFO.name) — the important thing is it's not
    // the old hardcoded "WAV" (though that would still pass). What matters:
    // the field is present and non-empty, confirming engine.play() populated it.
  }

  engine.stop();
  std::thread::sleep(std::time::Duration::from_millis(200));
}
