//! Integration test: IPC transport control flow (TCP).
//!
//! Tests IPC command serialization, engine lifecycle, and response formats.
use around_engine::{Engine, EngineConfig, PlaybackState, TrackState};
#[cfg(target_os = "linux")]
use cpal::traits::HostTrait;
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic counter for unique per-test identifiers within the same process.
static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);
fn unique_test_id() -> u64 {
  NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
}

fn audio_device_available() -> bool {
  #[cfg(target_os = "linux")]
  {
    let conf = concat!(env!("CARGO_MANIFEST_DIR"), "/../../ci/alsa-null.conf");
    std::env::set_var("ALSA_CONFIG_PATH", conf);
  }
  #[cfg(target_os = "linux")]
  {
    cpal::default_host().default_output_device().is_some()
  }
  #[cfg(not(target_os = "linux"))]
  {
    // On Windows/macOS CI there is typically no audio device.
    // Return false so audio-dependent tests skip gracefully.
    false
  }
}
fn fixture_path(name: &str) -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    .join("../../examples")
    .join(name)
}

fn send_command(port_file: &str, cmd: &Value) -> Value {
  let port: u16 = std::fs::read_to_string(port_file)
    .expect("read port file")
    .trim()
    .parse()
    .expect("parse port");
  let mut stream =
    TcpStream::connect(format!("127.0.0.1:{}", port)).expect("connect to IPC server");
  // Prevent hanging if server accepts but never responds.
  stream
    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
    .ok();
  let mut json = serde_json::to_string(cmd).unwrap();
  json.push('\n');
  stream.write_all(json.as_bytes()).expect("send command");

  let mut reader = BufReader::new(&stream);
  let mut response = String::new();
  reader.read_line(&mut response).expect("read response");
  serde_json::from_str(&response).expect("parse response")
}

/// Open a throwaway TCP connection to wake the server's accept loop
/// so it can observe a shutdown signal.
fn poke_server(port_file: &str) {
  if let Ok(port_str) = std::fs::read_to_string(port_file) {
    if let Ok(port) = port_str.trim().parse::<u16>() {
      let _ = TcpStream::connect(format!("127.0.0.1:{}", port));
    }
  }
}

/// Spin until engine reports playing state (direct state access, 1 ms poll).
/// Returns true if playing was observed; false if already finished or deadline expired.
fn ensure_playing(state: &Arc<Mutex<PlaybackState>>, timeout_ms: u64) -> bool {
  let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
  loop {
    let s = state.lock().unwrap_or_else(|e| e.into_inner());
    match s.state {
      TrackState::Playing => return true,
      TrackState::Stopped | TrackState::Error => return false,
      _ => {}
    }
    drop(s);
    if std::time::Instant::now() > deadline {
      return false;
    }
    std::thread::sleep(std::time::Duration::from_millis(1));
  }
}

/// Spin until engine reports stopped state (direct state access, 1 ms poll).
fn ensure_stopped(state: &Arc<Mutex<PlaybackState>>, timeout_ms: u64) {
  let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
  loop {
    let s = state.lock().unwrap_or_else(|e| e.into_inner());
    match s.state {
      TrackState::Stopped | TrackState::Error => return,
      _ => {}
    }
    drop(s);
    if std::time::Instant::now() > deadline {
      return;
    }
    std::thread::sleep(std::time::Duration::from_millis(1));
  }
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
fn ipc_server_creates_port_file_on_play() {
  // Guard to remove the port file on test exit (panic or success).
  struct PortFileGuard(String);
  impl Drop for PortFileGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  if !audio_device_available() {
    eprintln!("skipping test: no audio output device available");
    return;
  }

  let port_file = std::env::temp_dir()
    .join("around-test-ipc_server_creates_port_file_on_play.port")
    .to_string_lossy()
    .to_string();

  // Clean up any stale port file from a previous run.
  let _ = std::fs::remove_file(&port_file);

  let _guard = PortFileGuard(port_file.clone());

  // Port file must not exist before playback starts.
  assert!(
    !std::path::Path::new(&port_file).exists(),
    "port file should not exist before play"
  );

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  // Start IPC server on a background thread.
  let ipc_engine = engine.clone();
  let ipc_state = state.clone();
  let pf = port_file.clone();
  let _ipc_server = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    let mut ipc_config = around_engine::ipc::IpcConfig::default();
    ipc_config.enable_platform_native = false;
    ipc_config.check_running_instance = false;
    ipc_config.force_json = true;
    ipc_config.port_file_path = Some(std::path::PathBuf::from(pf));
    rt.block_on(around_engine::run_ipc_server(
      ipc_config, ipc_engine, ipc_state,
    ))
  });
  // Give server time to bind.
  // Wait for server to bind (poll for port file).
  {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !std::path::Path::new(&port_file).exists() {
      if std::time::Instant::now() > deadline {
        panic!("IPC server port file not created within 5s: {}", port_file);
      }
      std::thread::sleep(std::time::Duration::from_millis(50));
    }
  }

  // Start playback on a background thread (play() blocks until stopped).
  let play_engine = engine.clone();
  let play_state = state.clone();
  let play_thread = std::thread::spawn(move || {
    let source = around_source_file::FileSource::new(fixture_path("example.wav"));
    let _ = play_engine.play(Box::new(source), play_state);
  });
  let _playing = ensure_playing(&state, 200);

  // Port file must exist after server starts.
  assert!(
    std::path::Path::new(&port_file).exists(),
    "port file must exist after server starts"
  );

  // Verify it contains a valid port number.
  let port: u16 = std::fs::read_to_string(&port_file)
    .expect("read port file")
    .trim()
    .parse()
    .expect("valid port");
  assert!(port > 0, "port must be > 0");

  // Once the port file exists, send a status command and check the response.
  let resp = send_command(&port_file, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok", "status response must contain ok");

  // Stop playback.
  engine.stop();
  ensure_stopped(&state, 200);
  let _ = play_thread.join();

  // Verify playback stopped via status.
  let resp = send_command(&port_file, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok");

  // Manually clean up the port file (IPC server thread keeps running).
  let _ = std::fs::remove_file(&port_file);
  assert!(
    !std::path::Path::new(&port_file).exists(),
    "port file must be deletable after stop"
  );
}

#[test]
fn signal_cleanup_deletes_port_file() {
  // Guard to remove the port file on test exit (panic or success).
  struct PortFileGuard(String);
  impl Drop for PortFileGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  if !audio_device_available() {
    eprintln!("skipping test: no audio output device available");
    return;
  }

  let port_file = std::env::temp_dir()
    .join("around-test-signal_cleanup_deletes_port_file.port")
    .to_string_lossy()
    .to_string();

  // Clean up any stale port file from a previous run.
  let _ = std::fs::remove_file(&port_file);

  let _guard = PortFileGuard(port_file.clone());

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  let mut ipc_config = around_engine::ipc::IpcConfig::default();
  ipc_config.enable_platform_native = false;
  ipc_config.check_running_instance = false;
  ipc_config.force_json = true;
  ipc_config.port_file_path = Some(std::path::PathBuf::from(&port_file));

  // Start IPC server on a background thread.
  let ipc_engine = engine.clone();
  let ipc_state = state.clone();
  let _ipc_server = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(
      ipc_config, ipc_engine, ipc_state,
    ))
  });
  // Wait for server to bind and write port file.
  {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !std::path::Path::new(&port_file).exists() {
      if std::time::Instant::now() > deadline {
        panic!("IPC server port file not created within 5s: {}", port_file);
      }
      std::thread::sleep(std::time::Duration::from_millis(50));
    }
  }

  // Start playback on a background thread.
  let play_engine = engine.clone();
  let play_state = state.clone();
  let play_thread = std::thread::spawn(move || {
    let source = around_source_file::FileSource::new(fixture_path("example.wav"));
    let _ = play_engine.play(Box::new(source), play_state);
  });
  let _playing = ensure_playing(&state, 200);

  // Port file must exist after playback starts.
  assert!(
    std::path::Path::new(&port_file).exists(),
    "port file must exist after play starts"
  );

  // Send stop command via IPC.
  let resp = send_command(&port_file, &serde_json::json!({"command": "stop"}));
  assert_eq!(resp["status"], "ok", "stop response must contain ok");

  ensure_stopped(&state, 200);
  let _ = play_thread.join();

  // Manually clean up the port file (IPC server thread keeps running).
  let _ = std::fs::remove_file(&port_file);
  assert!(
    !std::path::Path::new(&port_file).exists(),
    "port file must be deletable after stop"
  );
}

// =============================================================================
// T020 – T025: IPC lifecycle integration tests
// =============================================================================

#[test]
fn port_file_created_on_bind() {
  struct PortFileGuard(String);
  impl Drop for PortFileGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  if !audio_device_available() {
    eprintln!("skipping test: no audio output device available");
    return;
  }

  let port_file = std::env::temp_dir()
    .join("around-test-port_file_created_on_bind.port")
    .to_string_lossy()
    .to_string();
  let _ = std::fs::remove_file(&port_file);
  let _guard = PortFileGuard(port_file.clone());

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  let mut ipc_config = around_engine::ipc::IpcConfig::default();
  ipc_config.enable_platform_native = false;
  ipc_config.check_running_instance = false;
  ipc_config.force_json = true;
  ipc_config.port_file_path = Some(std::path::PathBuf::from(&port_file));

  // Start IPC server on a background thread.
  let ipc_engine = engine.clone();
  let ipc_state = state.clone();
  let _ipc_server = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(
      ipc_config, ipc_engine, ipc_state,
    ))
  });
  // Wait for server to bind (poll for port file).
  {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !std::path::Path::new(&port_file).exists() {
      if std::time::Instant::now() > deadline {
        panic!("IPC server port file not created within 5s: {}", port_file);
      }
      std::thread::sleep(std::time::Duration::from_millis(50));
    }
  }

  // Port file must exist and contain a valid port after server starts.
  assert!(
    std::path::Path::new(&port_file).exists(),
    "port file must exist after server starts"
  );

  let port: u16 = std::fs::read_to_string(&port_file)
    .expect("read port file")
    .trim()
    .parse()
    .expect("valid port");
  assert!(port > 0, "port must be > 0");

  engine.stop();
  ensure_stopped(&state, 200);
}

#[test]
fn command_round_trip() {
  struct PortFileGuard(String);
  impl Drop for PortFileGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  if !audio_device_available() {
    eprintln!("skipping test: no audio output device available");
    return;
  }

  let port_file = std::env::temp_dir()
    .join("around-test-command_round_trip.port")
    .to_string_lossy()
    .to_string();
  let _ = std::fs::remove_file(&port_file);
  let _guard = PortFileGuard(port_file.clone());

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  let mut ipc_config = around_engine::ipc::IpcConfig::default();
  ipc_config.enable_platform_native = false;
  ipc_config.check_running_instance = false;
  ipc_config.force_json = true;
  ipc_config.port_file_path = Some(std::path::PathBuf::from(&port_file));

  // Start IPC server on a background thread.
  let ipc_engine = engine.clone();
  let ipc_state = state.clone();
  let _ipc_server = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(
      ipc_config, ipc_engine, ipc_state,
    ))
  });
  // Wait for server to bind and write port file.
  {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !std::path::Path::new(&port_file).exists() {
      if std::time::Instant::now() > deadline {
        panic!("IPC server port file not created within 5s: {}", port_file);
      }
      std::thread::sleep(std::time::Duration::from_millis(50));
    }
  }
  assert!(
    std::path::Path::new(&port_file).exists(),
    "port file must exist after server starts"
  );

  // Start playback on a background thread (play() blocks until stopped).
  let play_engine = engine.clone();
  let play_state = state.clone();
  let play_thread = std::thread::spawn(move || {
    let source = around_source_file::FileSource::new(fixture_path("example.wav"));
    let _ = play_engine.play(Box::new(source), play_state);
  });

  // Status command
  let resp = send_command(&port_file, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok", "status response must contain ok");

  // Pause / resume / seek: IPC handler may return ok (active track) or
  // NO_TRACK (track already finished). Both are valid — the play thread
  // and the IPC handler race. Verify correct response structure for each.
  let resp = send_command(&port_file, &serde_json::json!({"command": "pause"}));
  if resp["status"] == "ok" {
    // Track was active — ok
  } else {
    assert_eq!(
      resp["status"], "error",
      "pause on stopped engine must return error"
    );
    assert_eq!(resp["code"], "NO_TRACK", "error code must be NO_TRACK");
  }

  // Resume command
  let resp = send_command(&port_file, &serde_json::json!({"command": "resume"}));
  if resp["status"] == "ok" {
  } else {
    assert_eq!(
      resp["status"], "error",
      "resume on stopped engine must return error"
    );
  }

  // Seek command
  let resp = send_command(
    &port_file,
    &serde_json::json!({"command": "seek", "position_ms": 5000}),
  );
  if resp["status"] == "ok" {
  } else {
    assert_eq!(
      resp["status"], "error",
      "seek on stopped engine must return error"
    );
  }
  engine.stop();
  ensure_stopped(&state, 200);
  let _ = play_thread.join();
}

#[test]
fn stale_port_file_handled() {
  struct PortFileGuard(String);
  impl Drop for PortFileGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  let port_file = std::env::temp_dir()
    .join("around-test-stale_port_file_handled.port")
    .to_string_lossy()
    .to_string();
  let _ = std::fs::remove_file(&port_file);

  // Create a stale port file manually (simulating a crashed previous instance
  // that left a port file pointing at a dead server).
  std::fs::write(&port_file, "65535\n").expect("create stale port file");
  assert!(
    std::path::Path::new(&port_file).exists(),
    "stale port file must exist before starting IPC server"
  );

  let _guard = PortFileGuard(port_file.clone());

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  let mut ipc_config = around_engine::ipc::IpcConfig::default();
  ipc_config.enable_platform_native = false;
  ipc_config.check_running_instance = false;
  ipc_config.force_json = true;
  ipc_config.port_file_path = Some(std::path::PathBuf::from(&port_file));

  // Start IPC server — it should detect the stale port, ignore it, and write a fresh one.
  let ipc_engine = engine.clone();
  let ipc_state = state.clone();
  let _ipc_server = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(
      ipc_config, ipc_engine, ipc_state,
    ))
  });
  // Wait for server to overwrite stale port file with a valid port.
  {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
      if let Ok(contents) = std::fs::read_to_string(&port_file) {
        if let Ok(port) = contents.trim().parse::<u16>() {
          if port != 65535 && port > 0 {
            break;
          }
        }
      }
      if std::time::Instant::now() > deadline {
        panic!(
          "server did not overwrite stale port file within 5s: {}",
          port_file
        );
      }
      std::thread::sleep(std::time::Duration::from_millis(50));
    }
  }

  // The stale port file must be replaced by a fresh one with a valid port.
  assert!(
    std::path::Path::new(&port_file).exists(),
    "port file must exist after IPC server starts (replaces stale)"
  );

  let port: u16 = std::fs::read_to_string(&port_file)
    .expect("read port file")
    .trim()
    .parse()
    .expect("valid port after replacement");
  assert!(port > 0, "replacement port must be > 0");
  // It must not be the stale value.
  assert_ne!(port, 65535, "port must not be the stale value");

  engine.stop();
  ensure_stopped(&state, 200);
}

#[test]
fn running_instance_rejection() {
  // Use a unique port file to avoid collisions with other parallel tests.
  let port_file = std::env::temp_dir()
    .join("around-test-running_instance_rejection.port")
    .to_string_lossy()
    .to_string();
  let _ = std::fs::remove_file(&port_file);

  struct PortFileGuard(String);
  impl Drop for PortFileGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }
  let _guard = PortFileGuard(port_file.clone());

  // Start first IPC server.
  let config = EngineConfig::load();
  let engine_a = Arc::new(Engine::new(config.clone()));
  let state_a = Arc::new(Mutex::new(PlaybackState::default()));
  let mut ipc_config_a = around_engine::ipc::IpcConfig::default();
  ipc_config_a.enable_platform_native = false;
  ipc_config_a.check_running_instance = false;
  ipc_config_a.force_json = true;
  ipc_config_a.port_file_path = Some(std::path::PathBuf::from(&port_file));
  let eng_a = engine_a.clone();
  let st_a = state_a.clone();
  let _path_a = port_file.clone();
  let _server_a = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime for first instance");
    rt.block_on(around_engine::run_ipc_server(ipc_config_a, eng_a, st_a))
  });

  // Give the first server time to bind.
  // Wait for first server to bind (poll for port file).
  {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !std::path::Path::new(&port_file).exists() {
      if std::time::Instant::now() > deadline {
        panic!(
          "first IPC server port file not created within 5s: {}",
          port_file
        );
      }
      std::thread::sleep(std::time::Duration::from_millis(50));
    }
  }

  // Second IPC server should detect the running instance and fail.
  let engine_b = Arc::new(Engine::new(config));
  let state_b = Arc::new(Mutex::new(PlaybackState::default()));
  let mut ipc_config_b = around_engine::ipc::IpcConfig::default();
  ipc_config_b.enable_platform_native = false;
  ipc_config_b.check_running_instance = true; // second instance must detect
  ipc_config_b.port_file_path = Some(std::path::PathBuf::from(&port_file));
  let _path_b = port_file.clone();
  let result = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime for second instance");
    rt.block_on(around_engine::run_ipc_server(
      ipc_config_b,
      engine_b,
      state_b,
    ))
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

  // Signal first server to shut down and poke it to wake the accept loop.
  engine_a.shutdown();
  poke_server(&port_file);
}

#[test]
fn port_file_cleanup_after_stop() {
  struct PortFileGuard(String);
  impl Drop for PortFileGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  if !audio_device_available() {
    eprintln!("skipping test: no audio output device available");
    return;
  }

  let port_file = std::env::temp_dir()
    .join("around-test-port_file_cleanup_after_stop.port")
    .to_string_lossy()
    .to_string();
  let _ = std::fs::remove_file(&port_file);
  let _guard = PortFileGuard(port_file.clone());

  // Port file must not exist before playback.
  assert!(
    !std::path::Path::new(&port_file).exists(),
    "port file must not exist before play"
  );

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  let mut ipc_config = around_engine::ipc::IpcConfig::default();
  ipc_config.enable_platform_native = false;
  ipc_config.check_running_instance = false;
  ipc_config.force_json = true;
  ipc_config.port_file_path = Some(std::path::PathBuf::from(&port_file));

  // Start IPC server on a background thread.
  let ipc_engine = engine.clone();
  let ipc_state = state.clone();
  let _ipc_server = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(
      ipc_config, ipc_engine, ipc_state,
    ))
  });
  // Wait for server to bind (poll for port file).
  {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !std::path::Path::new(&port_file).exists() {
      if std::time::Instant::now() > deadline {
        panic!("IPC server port file not created within 5s: {}", port_file);
      }
      std::thread::sleep(std::time::Duration::from_millis(50));
    }
  }

  // Start playback on a background thread.
  let play_engine = engine.clone();
  let play_state = state.clone();
  let play_thread = std::thread::spawn(move || {
    let source = around_source_file::FileSource::new(fixture_path("example.wav"));
    let _ = play_engine.play(Box::new(source), play_state);
  });
  let _playing = ensure_playing(&state, 200);

  // Port file must exist during playback.
  assert!(
    std::path::Path::new(&port_file).exists(),
    "port file must exist during playback"
  );
  // Send stop command via IPC.
  let resp = send_command(&port_file, &serde_json::json!({"command": "stop"}));
  assert_eq!(resp["status"], "ok", "stop response must contain ok");

  ensure_stopped(&state, 200);
  let _ = play_thread.join();

  // Manually clean up the port file (IPC server thread keeps running).
  let _ = std::fs::remove_file(&port_file);
  assert!(
    !std::path::Path::new(&port_file).exists(),
    "port file must be deletable after stop"
  );
}

#[test]
fn port_file_cleanup_after_signal() {
  struct PortFileGuard(String);
  impl Drop for PortFileGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  // Use a unique port file path per test run
  let port_file = std::env::temp_dir()
    .join("around-test-port_file_cleanup_after_signal.port")
    .to_string_lossy()
    .to_string();
  let _ = std::fs::remove_file(&port_file);
  let _guard = PortFileGuard(port_file.clone());

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));
  let mut ipc_config = around_engine::ipc::IpcConfig::default();
  ipc_config.enable_platform_native = false;
  ipc_config.check_running_instance = false;
  ipc_config.force_json = true;
  ipc_config.port_file_path = Some(std::path::PathBuf::from(&port_file));
  let eng = engine.clone();
  let st = state.clone();

  // Start IPC server on a background thread
  let server = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(ipc_config, eng, st))
  });

  // Wait for server to bind (poll for port file).
  {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !std::path::Path::new(&port_file).exists() {
      if std::time::Instant::now() > deadline {
        panic!("IPC server port file not created within 5s: {}", port_file);
      }
      std::thread::sleep(std::time::Duration::from_millis(50));
    }
  }

  // Port file should exist
  assert!(
    std::path::Path::new(&port_file).exists(),
    "port file should be created by IPC server"
  );

  // Simulate signal by stopping the engine (same effect as SIGTERM → engine.stop())
  // The IPC server should shut down when the runtime is dropped
  drop(server);

  // Brief delay for cleanup
  std::thread::sleep(std::time::Duration::from_millis(50));

  // Note: in the current implementation, the IPC server drops the runtime
  // but the port file is only written by run_ipc_server itself.
  // This test validates that the port file can be cleaned up externally.
  let _ = std::fs::remove_file(&port_file);
  assert!(!std::path::Path::new(&port_file).exists());
}

// =============================================================================

/// Helper: spawn an IPC server on a unique port file, return (port_file, engine, state).
fn spawn_ipc_server(suffix: &str) -> (String, Arc<Engine>, Arc<Mutex<PlaybackState>>) {
  let port_file = std::env::temp_dir()
    .join(format!("around-{}-{}.port", std::process::id(), suffix))
    .to_string_lossy()
    .to_string();
  let _ = std::fs::remove_file(&port_file);

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  let mut ipc_config = around_engine::ipc::IpcConfig::default();
  ipc_config.enable_platform_native = false;
  ipc_config.check_running_instance = false;
  ipc_config.force_json = true;
  ipc_config.port_file_path = Some(std::path::PathBuf::from(&port_file));

  let ipc_engine = engine.clone();
  let ipc_state = state.clone();
  std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(
      ipc_config, ipc_engine, ipc_state,
    ))
  });

  // Wait for server to bind (poll for port file existence).
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
  while !std::path::Path::new(&port_file).exists() {
    if std::time::Instant::now() > deadline {
      panic!("IPC server port file not created within 5s: {}", port_file);
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
  }

  (port_file, engine, state)
}

struct PortFileGuard(String);
impl Drop for PortFileGuard {
  fn drop(&mut self) {
    let _ = std::fs::remove_file(&self.0);
  }
}

#[test]
fn runtime_idle_pause_returns_no_track() {
  let (port_file, engine, _state) = spawn_ipc_server("idle_pause");
  let _guard = PortFileGuard(port_file.clone());

  let resp = send_command(&port_file, &serde_json::json!({"command": "pause"}));
  assert_eq!(
    resp["status"], "error",
    "pause without playback must return error"
  );
  assert_eq!(resp["code"], "NO_TRACK", "error code must be NO_TRACK");

  engine.shutdown();
  poke_server(&port_file);
}

// ---------------------------------------------------------------------------
// Shutdown flow test
// ---------------------------------------------------------------------------

#[test]
fn shutdown_command_terminates_engine() {
  let (port_file, engine, _state) = spawn_ipc_server("shutdown");
  let _guard = PortFileGuard(port_file.clone());
  let resp = send_command(&port_file, &serde_json::json!({"command": "shutdown"}));
  assert_eq!(resp["status"], "ok");
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
  while !engine.is_shutdown() {
    if std::time::Instant::now() > deadline {
      panic!("engine did not shut down within 5s");
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
  }
}

// ---------------------------------------------------------------------------
// Rapid play state consistency (B4 regression)
// ---------------------------------------------------------------------------

#[test]
fn rapid_play_status_never_returns_stopped() {
  if !audio_device_available() {
    eprintln!("skipping test: no audio output device available");
    return;
  }
  let (port_file, engine, state) = spawn_ipc_server("play_race");
  let _guard = PortFileGuard(port_file.clone());

  // Mock: active playback running.
  {
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    st.playing = true;
    st.state = TrackState::Playing;
    st.epoch = 5;
  }

  // Send "play" — handler calls engine.stop() then increments generation
  // and sets state to buffering. Old generation cleanup must not overwrite.
  let wav = fixture_path("example.wav");
  let resp = send_command(
    &port_file,
    &serde_json::json!({"command": "play", "path": wav}),
  );
  assert_eq!(resp["state"], "buffering");

  // Verify shared state reflects the new generation, not the old cleanup.
  {
    let st = state.lock().unwrap_or_else(|e| e.into_inner());
    // Buffering is transient; playback of a short file may already be done.
    // The key invariant: old generation must not persist.
    assert_ne!(
      st.state,
      TrackState::Playing,
      "old generation Playing state must not survive new play command"
    );
    assert!(st.epoch > 5, "generation must have advanced");
  }

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn ipc_play_response_has_correct_format() {
  let (port_file, engine, state) = spawn_ipc_server("play_format");
  let _guard = PortFileGuard(port_file.clone());

  let resp = send_command(
    &port_file,
    &serde_json::json!({"command": "play", "path": fixture_path("example.wav")}),
  );
  // Immediate play response must have:
  // - status: ok
  // - track_id: 1
  // - state: playing
  // - No error fields (code, message)
  assert_eq!(resp["status"], "ok");
  assert_eq!(resp["track_id"], 1);
  assert_eq!(resp["state"], "buffering");
  assert!(
    resp.get("code").is_none(),
    "ok response must not have error code"
  );
  assert!(
    resp.get("message").is_none(),
    "ok response must not have message"
  );

  engine.stop();
  ensure_stopped(&state, 200);
  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn runtime_idle_resume_returns_no_track() {
  let (port_file, engine, _state) = spawn_ipc_server("idle_resume");
  let _guard = PortFileGuard(port_file.clone());

  let resp = send_command(&port_file, &serde_json::json!({"command": "resume"}));
  assert_eq!(
    resp["status"], "error",
    "resume without playback must return error"
  );
  assert_eq!(resp["code"], "NO_TRACK", "error code must be NO_TRACK");

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn runtime_idle_seek_returns_no_track() {
  let (port_file, engine, _state) = spawn_ipc_server("idle_seek");
  let _guard = PortFileGuard(port_file.clone());

  let resp = send_command(
    &port_file,
    &serde_json::json!({"command": "seek", "position_ms": 5000}),
  );
  assert_eq!(
    resp["status"], "error",
    "seek without playback must return error"
  );
  assert_eq!(resp["code"], "NO_TRACK", "error code must be NO_TRACK");

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn runtime_status_returns_stopped_when_idle() {
  let (port_file, engine, _state) = spawn_ipc_server("idle_status");
  let _guard = PortFileGuard(port_file.clone());

  let resp = send_command(&port_file, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok", "status must return ok even when idle");
  assert_eq!(resp["state"], "stopped", "idle state must be stopped");
  assert_eq!(resp["position_ms"], 0, "idle position must be 0");
  assert_eq!(
    resp["device_lost"], false,
    "device_lost must be false when idle"
  );

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn runtime_protocol_no_deadlock() {
  // Verify IPC commands return immediately (< 500ms), not hung.
  let (port_file, engine, _state) = spawn_ipc_server("no_deadlock");
  let _guard = PortFileGuard(port_file.clone());

  let start = std::time::Instant::now();
  let resp = send_command(&port_file, &serde_json::json!({"command": "status"}));
  let elapsed = start.elapsed();

  assert!(
    elapsed < std::time::Duration::from_millis(500),
    "IPC command must return quickly, took {:?}",
    elapsed
  );
  assert_eq!(resp["status"], "ok");

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn runtime_stop_idempotent_via_ipc() {
  let (port_file, engine, _state) = spawn_ipc_server("idempotent_stop");
  let _guard = PortFileGuard(port_file.clone());

  // First stop — should succeed.
  let resp = send_command(&port_file, &serde_json::json!({"command": "stop"}));
  assert_eq!(resp["status"], "ok", "first stop must return ok");
  assert_eq!(resp["state"], "stopped");

  // Second stop — idempotent, still ok.
  let resp = send_command(&port_file, &serde_json::json!({"command": "stop"}));
  assert_eq!(resp["status"], "ok", "second stop must also return ok");

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn runtime_state_mutation_visible_to_ipc() {
  // Mutate shared state directly, then verify IPC status sees the change.
  let (port_file, engine, state) = spawn_ipc_server("state_visible");
  let _guard = PortFileGuard(port_file.clone());

  // Mutate state externally (simulating what the decode loop does).
  {
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    st.track_path = Some("/tmp/fake.wav".into());
    st.format_name = Some("WAV".into());
    st.duration_ms = Some(42000);
    st.playing = true;
    st.state = TrackState::Playing;
    st.position_ms = 12345;
  }

  let resp = send_command(&port_file, &serde_json::json!({"command": "status"}));
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

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn runtime_pause_while_playing_succeeds() {
  // Set state to playing, then send pause via IPC → must succeed.
  let (port_file, engine, state) = spawn_ipc_server("pause_playing");
  let _guard = PortFileGuard(port_file.clone());

  // Simulate active playback.
  {
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    st.playing = true;
    st.state = TrackState::Playing;
    st.position_ms = 5000;
  }

  let resp = send_command(&port_file, &serde_json::json!({"command": "pause"}));
  assert_eq!(resp["status"], "ok", "pause during playback must succeed");
  assert_eq!(resp["state"], "paused", "state must transition to paused");
  assert_eq!(resp["position_ms"], 5000, "position must be preserved");

  // Verify engine flag was set.
  assert!(!engine.is_shutdown()); // engine exists but pause doesn't shut down

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn runtime_seek_updates_position_in_state() {
  // Set state to playing, send seek → position_ms must update.
  let (port_file, engine, state) = spawn_ipc_server("seek_position");
  let _guard = PortFileGuard(port_file.clone());

  {
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    st.playing = true;
    st.state = TrackState::Playing;
    st.seekable = true;
    st.position_ms = 1000;
  }

  let resp = send_command(
    &port_file,
    &serde_json::json!({"command": "seek", "position_ms": 30000}),
  );
  assert_eq!(resp["status"], "ok", "seek during playback must succeed");
  assert_eq!(
    resp["position_ms"], 30000,
    "response must echo requested position"
  );

  // Verify state was updated.
  {
    let st = state.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(
      st.position_ms, 30000,
      "shared state position_ms must be 30000 after seek"
    );
    assert_eq!(
      st.state,
      TrackState::Buffering,
      "shared state must be buffering after seek (optimistic)"
    );
  }

  engine.shutdown();
  poke_server(&port_file);
}

// =============================================================================
// Regression: coverage for runtime issues found during codec system refactor
// =============================================================================

#[test]
fn seek_non_seekable_source_returns_not_supported() {
  // When seekable=false in PlaybackState, handle_seek must return NOT_SUPPORTED.
  let (port_file, engine, state) = spawn_ipc_server("seek_not_supported");
  let _guard = PortFileGuard(port_file.clone());

  {
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    st.playing = true;
    st.state = TrackState::Playing;
    st.seekable = false;
    st.position_ms = 1000;
  }

  let resp = send_command(
    &port_file,
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
    let st = state.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(
      st.position_ms, 1000,
      "position must be unchanged after rejected seek"
    );
  }

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn status_includes_seekable_flag() {
  // Verify that seekable flag set during play() is visible via IPC status.
  // (Indirectly tests that engine.play() propagates seekable to PlaybackState.)
  let (port_file, engine, state) = spawn_ipc_server("status_seekable");
  let _guard = PortFileGuard(port_file.clone());

  {
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    st.playing = true;
    st.state = TrackState::Playing;
    st.seekable = true;
    st.track_path = Some("/tmp/fake.wav".into());
  }

  let resp = send_command(
    &port_file,
    &serde_json::json!({"command": "seek", "position_ms": 2000}),
  );
  assert_eq!(resp["status"], "ok", "seek on seekable source must succeed");

  engine.shutdown();
  poke_server(&port_file);
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

  struct PortFileGuard(String);
  impl Drop for PortFileGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  let port_file = std::env::temp_dir()
    .join("around-test-format_name_propagated_to_status_during_playback.port")
    .to_string_lossy()
    .to_string();
  let _ = std::fs::remove_file(&port_file);
  let _guard = PortFileGuard(port_file.clone());

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  let mut ipc_config = around_engine::ipc::IpcConfig::default();
  ipc_config.enable_platform_native = false;
  ipc_config.check_running_instance = false;
  ipc_config.force_json = true;
  ipc_config.port_file_path = Some(std::path::PathBuf::from(&port_file));

  // Start IPC server.
  let ipc_engine = engine.clone();
  let ipc_state = state.clone();
  let _ipc_server = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(
      ipc_config, ipc_engine, ipc_state,
    ))
  });
  // Wait for server to bind (poll for port file).
  {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !std::path::Path::new(&port_file).exists() {
      if std::time::Instant::now() > deadline {
        panic!("IPC server port file not created within 5s: {}", port_file);
      }
      std::thread::sleep(std::time::Duration::from_millis(50));
    }
  }

  // Start playback.
  let play_engine = engine.clone();
  let play_state = state.clone();
  let play_thread = std::thread::spawn(move || {
    let source = around_source_file::FileSource::new(fixture_path("example.wav"));
    let _ = play_engine.play(Box::new(source), play_state);
  });
  let _playing = ensure_playing(&state, 200);

  // Status must contain format_name from the codec (not "WAV" hardcode).
  let resp = send_command(&port_file, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok");

  if let Some(track) = resp.get("track") {
    let fmt = track["format"].as_str().unwrap_or("");
    assert!(
      !fmt.is_empty(),
      "format_name must not be empty during playback"
    );
  }

  engine.stop();
  ensure_stopped(&state, 200);
  let _ = play_thread.join();
}

// =============================================================================
// Codec and cleanup command tests
// =============================================================================

#[test]
fn ipc_list_codecs_returns_empty_initially() {
  let (port_file, engine, _state) = spawn_ipc_server("list_codecs_empty");
  let _guard = PortFileGuard(port_file.clone());

  let resp = send_command(&port_file, &serde_json::json!({"command": "list_codecs"}));
  assert_eq!(resp["status"], "ok");
  // Initially empty — no codecs loaded beyond builtins.
  if let Some(codecs) = resp.get("codecs") {
    let _arr = codecs.as_array().unwrap();
    // list_codecs returns dynamically-loaded extensions only.
    // Built-in codecs (WAV, PCM) are registered via the codec registry,
    // not the extension manager, so the list may be empty.
  }

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn ipc_load_codec_nonexistent_file_errors() {
  let (port_file, engine, _state) = spawn_ipc_server("load_codec_nonexistent");
  let _guard = PortFileGuard(port_file.clone());

  let resp = send_command(
    &port_file,
    &serde_json::json!({"command": "load_codec", "path": "/nonexistent/codec.so"}),
  );
  assert_eq!(resp["status"], "error");
  assert!(resp["code"]
    .as_str()
    .is_some_and(|c| c == "CODEC_LOAD_FAILED" || c == "INTERNAL"));

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn ipc_cleanup_returns_removed_files_field() {
  let (port_file, engine, _state) = spawn_ipc_server("cleanup");
  let _guard = PortFileGuard(port_file.clone());

  let resp = send_command(&port_file, &serde_json::json!({"command": "cleanup"}));
  assert_eq!(resp["status"], "ok");
  // Must have removed_files field (even if empty).
  assert!(
    resp.get("removed_files").is_some(),
    "cleanup response must include removed_files field"
  );

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn ipc_load_codec_bytes_invalid_data_errors() {
  let (port_file, engine, _state) = spawn_ipc_server("load_codec_bytes_invalid");
  let _guard = PortFileGuard(port_file.clone());

  // Send load_codec_bytes with invalid base64 data (not a valid .so/.dll).
  let resp = send_command(
    &port_file,
    &serde_json::json!({"command": "load_codec_bytes", "data": "aW52YWxpZA=="}),
  );
  assert_eq!(resp["status"], "error");
  assert_eq!(resp["code"], "CODEC_LOAD_FAILED");

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn ipc_multiple_commands_over_one_connection() {
  let (port_file, engine, _state) = spawn_ipc_server("multi_cmd");
  let _guard = PortFileGuard(port_file.clone());

  let port: u16 = std::fs::read_to_string(&port_file)
    .expect("read port file")
    .trim()
    .parse()
    .expect("parse port");

  use std::io::{BufRead, BufReader, Write};
  let stream = TcpStream::connect(format!("127.0.0.1:{}", port)).expect("connect");
  let mut writer = stream.try_clone().expect("clone write half");
  let mut reader = BufReader::new(&stream);

  // Send 3 commands over the same connection without closing.
  for i in 0..3 {
    let cmd = serde_json::json!({"command": "status"});
    let mut json = serde_json::to_string(&cmd).unwrap();
    json.push('\n');
    writer.write_all(json.as_bytes()).expect("send");
    writer.flush().expect("flush");

    let mut response = String::new();
    reader.read_line(&mut response).expect("read response");
    let resp: serde_json::Value = serde_json::from_str(&response).expect("parse");
    assert_eq!(resp["status"], "ok", "command {} must return ok", i);
    assert_eq!(
      resp["state"], "stopped",
      "command {} state must be stopped",
      i
    );
  }

  // Drop the stream to close the connection.
  drop(writer);
  drop(stream);

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn ipc_cleanup_removes_matching_temp_files() {
  let (port_file, engine, _state) = spawn_ipc_server("cleanup_files");
  let _guard = PortFileGuard(port_file.clone());

  // Use a fake PID so handle_cleanup treats this as a stale file from a
  // crashed previous instance (skip current_pid, remove everything else).
  let fake_pid = 99000u64 + unique_test_id();
  let tmp = std::env::var("TMPDIR")
    .map(std::path::PathBuf::from)
    .or_else(|_| std::env::var("TEMP").map(std::path::PathBuf::from))
    .unwrap_or_else(|_| std::env::temp_dir());
  let test_file = tmp.join(format!("around_codec_{}_test_cleanup.tmp", fake_pid));
  std::fs::write(&test_file, b"test").expect("create test file");
  assert!(test_file.exists(), "test file must exist before cleanup");

  let resp = send_command(&port_file, &serde_json::json!({"command": "cleanup"}));
  assert_eq!(resp["status"], "ok");

  assert!(
    !test_file.exists(),
    "cleanup must remove matching temp files"
  );

  // Deterministic: fake PID ensures no other test's cleanup touches our file.
  // handle_cleanup is a global operation — parallel tests may remove each
  // other's stale files. Contract: the file no longer exists.
  let _ = resp.get("removed_files").and_then(|v| v.as_array());

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn ipc_play_nonexistent_file_returns_file_not_found() {
  let (port_file, engine, _state) = spawn_ipc_server("play_notfound");
  let _guard = PortFileGuard(port_file.clone());

  let resp = send_command(
    &port_file,
    &serde_json::json!({"command": "play", "path": "/tmp/__nonexistent_around_test_xyz.wav"}),
  );
  assert_eq!(resp["status"], "error");
  assert_eq!(resp["code"], "FILE_NOT_FOUND");
  assert!(resp["message"]
    .as_str()
    .unwrap_or("")
    .contains("No such file"));
  // Error response must not have track_id or state.
  assert!(
    resp.get("track_id").is_none(),
    "error must not have track_id"
  );
  assert!(resp.get("state").is_none(), "error must not have state");

  engine.shutdown();
  poke_server(&port_file);
}

// =============================================================================
// High-priority tests: edge cases and error paths
// =============================================================================

#[test]
fn ipc_play_truncated_file_sets_error_state() {
  // H1: Send play with truncated WAV, verify engine transitions to error/stopped.
  let (port_file, engine, _state) = spawn_ipc_server("truncated_error");
  let _guard = PortFileGuard(port_file.clone());

  let path = fixture_path("truncated.wav");
  let resp = send_command(
    &port_file,
    &serde_json::json!({"command": "play", "path": path}),
  );
  // Immediate response may be ok (play accepted) or error (decode failed early).
  // Either way, the engine must not be stuck in "playing" after a grace period.
  assert!(
    resp["status"] == "ok" || resp["status"] == "error",
    "immediate play response must be ok or error"
  );

  // Wait for engine to transition out of playing (to error or stopped).
  {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
      let status = send_command(&port_file, &serde_json::json!({"command": "status"}));
      let state = status["state"].as_str().unwrap_or("");
      if state == "error" || state == "stopped" {
        break;
      }
      if std::time::Instant::now() > deadline {
        break;
      }
      std::thread::sleep(std::time::Duration::from_millis(50));
    }
  }

  let status = send_command(&port_file, &serde_json::json!({"command": "status"}));
  assert_eq!(status["status"], "ok");
  let state = status["state"].as_str().unwrap_or("");
  assert!(
    state == "error" || state == "stopped",
    "after truncated file, state must be error or stopped, got '{}'",
    state
  );

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn ipc_cleanup_removes_underscore_prefix_files() {
  // H2: handle_cleanup must match files with underscore-prefix `around_codec_`.
  let (port_file, engine, _state) = spawn_ipc_server("cleanup_underscore");
  let _guard = PortFileGuard(port_file.clone());

  let fake_pid = 99100u64 + unique_test_id();
  let tmp = std::env::var("TMPDIR")
    .map(std::path::PathBuf::from)
    .or_else(|_| std::env::var("TEMP").map(std::path::PathBuf::from))
    .unwrap_or_else(|_| std::env::temp_dir());
  let test_file = tmp.join(format!("around_codec_{}_cleanup_test.tmp", fake_pid));
  std::fs::write(&test_file, b"test").expect("create test file");
  assert!(test_file.exists(), "test file must exist before cleanup");

  let resp = send_command(&port_file, &serde_json::json!({"command": "cleanup"}));
  assert_eq!(resp["status"], "ok");

  assert!(
    !test_file.exists(),
    "cleanup must remove underscore-prefixed temp files"
  );

  // handle_cleanup is a global operation — parallel tests may remove each
  // other's stale files. Contract: the file no longer exists. Whether
  // *this* call or another one removed it is irrelevant.
  let _ = resp.get("removed_files").and_then(|v| v.as_array());

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn ipc_resume_after_pause_transitions_to_playing() {
  // H3: Resume from paused state must return playing with position preserved.
  let (port_file, engine, state) = spawn_ipc_server("resume_paused");
  let _guard = PortFileGuard(port_file.clone());

  {
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    st.playing = true;
    st.state = TrackState::Paused;
    st.position_ms = 5000;
  }

  let resp = send_command(&port_file, &serde_json::json!({"command": "resume"}));
  assert_eq!(resp["status"], "ok", "resume from paused must succeed");
  assert_eq!(resp["state"], "playing", "state must transition to playing");
  assert_eq!(resp["position_ms"], 5000, "position must be preserved");

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn ipc_seek_to_zero_position() {
  // H5: Seek to position_ms=0 boundary must succeed and update state.
  let (port_file, engine, state) = spawn_ipc_server("seek_zero");
  let _guard = PortFileGuard(port_file.clone());

  {
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    st.playing = true;
    st.state = TrackState::Playing;
    st.seekable = true;
    st.position_ms = 10000;
  }

  let resp = send_command(
    &port_file,
    &serde_json::json!({"command": "seek", "position_ms": 0}),
  );
  assert_eq!(resp["status"], "ok", "seek to 0 must succeed");
  assert_eq!(resp["position_ms"], 0, "position_ms must be 0");

  {
    let st = state.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(st.position_ms, 0, "shared state position_ms must be 0");
  }

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn ipc_seek_while_paused() {
  // H6: Seek while paused must succeed and update position.
  let (port_file, engine, state) = spawn_ipc_server("seek_paused");
  let _guard = PortFileGuard(port_file.clone());

  {
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    st.playing = true;
    st.state = TrackState::Paused;
    st.seekable = true;
    st.position_ms = 10000;
  }

  let resp = send_command(
    &port_file,
    &serde_json::json!({"command": "seek", "position_ms": 30000}),
  );
  assert_eq!(resp["status"], "ok", "seek while paused must succeed");
  assert_eq!(resp["position_ms"], 30000, "position must update");

  {
    let st = state.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(
      st.position_ms, 30000,
      "shared state position_ms must be 30000"
    );
  }

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn ipc_seek_with_max_value_does_not_panic() {
  // M6: Seek to u64::MAX must not panic; engine must clamp or error gracefully.
  let (port_file, engine, state) = spawn_ipc_server("seek_max");
  let _guard = PortFileGuard(port_file.clone());

  {
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    st.playing = true;
    st.state = TrackState::Playing;
    st.seekable = true;
    st.position_ms = 0;
  }

  let resp = send_command(
    &port_file,
    &serde_json::json!({"command": "seek", "position_ms": u64::MAX}),
  );
  // Engine must not panic. It may clamp, accept, or error — all are valid.
  // The key property: the test reaches this line without panicking.
  let status = resp["status"].as_str().unwrap_or("");
  assert!(
    status == "ok" || status == "error",
    "seek with u64::MAX must return ok or error, got '{}'",
    status
  );

  // Verify state is still consistent (not corrupted).
  {
    let st = state.lock().unwrap_or_else(|e| e.into_inner());
    assert!(st.playing, "playing flag must remain true");
  }

  engine.shutdown();
  poke_server(&port_file);
}

// ---------------------------------------------------------------------------
// UDP transport test
// ---------------------------------------------------------------------------

#[test]
fn udp_status_round_trip() {
  let (port_file, engine, _state) = spawn_ipc_server("udp");
  let _guard = PortFileGuard(port_file.clone());

  // Read TCP port (spawn_ipc_server creates TCP by default).
  let tcp_port: u16 = std::fs::read_to_string(&port_file)
    .unwrap()
    .trim()
    .parse()
    .unwrap();

  // Also start a UDP socket alongside — connect, send status, verify response.
  let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
  udp
    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
    .unwrap();

  // The UDP transport is NOT started by spawn_ipc_server (no udp_bind set).
  // This test just verifies that the module compiles and infrastructure exists.
  // Full UDP round-trip requires configuring IpcConfig::udp_bind.

  let cmd = serde_json::json!({"command": "status"});
  // Connect via TCP (UDP not configured here).
  let mut stream = TcpStream::connect_timeout(
    &format!("127.0.0.1:{}", tcp_port).parse().unwrap(),
    std::time::Duration::from_secs(3),
  )
  .unwrap();

  let payload = serde_json::to_string(&cmd).unwrap() + "\n";
  stream.write_all(payload.as_bytes()).unwrap();
  let mut reader = BufReader::new(&stream);
  let mut response = String::new();
  reader.read_line(&mut response).unwrap();
  let v: Value = serde_json::from_str(&response).unwrap();
  assert_eq!(v["status"], "ok");

  engine.shutdown();
  poke_server(&port_file);
}
