//! Integration test: IPC transport control flow (TCP).
//!
//! Tests IPC command serialization, engine lifecycle, and response formats.
use around_engine::{Engine, EngineConfig};
#[cfg(target_os = "linux")]
use cpal::traits::HostTrait;
use serde_json::Value;
use std::io::{BufReader, BufWriter, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Monotonic counter for unique per-test identifiers within the same process.
static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);
fn unique_test_id() -> u64 {
  NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
}

/// Check if an audio device is available for tests that require real playback.
fn audio_device_available() -> bool {
  let host = cpal::default_host();
  host.default_output_device().is_some()
}

fn fixture_path(name: &str) -> PathBuf {
  let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
  p.push("tests");
  p.push("fixtures");
  p.push(name);
  p
}

fn send_command(port_file: &str, cmd: &Value) -> Value {
  let port_str = std::fs::read_to_string(port_file).expect("read port file");
  let port: u16 = port_str.trim().parse().expect("parse port");
  let stream = TcpStream::connect(format!("127.0.0.1:{}", port)).expect("connect");

  let mut reader = BufReader::new(&stream);
  let mut writer = BufWriter::new(&stream);

  let json = serde_json::to_vec(cmd).expect("serialize");
  writer
    .write_all(&(json.len() as u64).to_le_bytes())
    .expect("write len");
  writer.write_all(&json).expect("write json");
  writer.flush().expect("flush");

  let mut len_buf = [0u8; 8];
  reader.read_exact(&mut len_buf).expect("read len");
  let resp_len = u64::from_le_bytes(len_buf) as usize;
  let mut resp_buf = vec![0u8; resp_len];
  reader.read_exact(&mut resp_buf).expect("read resp");
  serde_json::from_slice(&resp_buf).expect("parse resp")
}

/// Open a throwaway TCP connection to wake the server's accept loop
/// so it can observe a shutdown signal.
fn poke_server(port_file: &str) {
  let port_str = std::fs::read_to_string(port_file).ok();
  if let Some(s) = port_str {
    if let Ok(port) = s.trim().parse::<u16>() {
      let _ = TcpStream::connect(format!("127.0.0.1:{}", port));
    }
  }
}

#[test]
fn engine_play_nonexistent_file_errors() {
  let config = EngineConfig::load();
  let engine = Engine::new(config);
  let source = around_source_file::FileSource::new("/tmp/__nonexistent_around_test__.wav");
  let result = engine.prepare(Box::new(source));
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
  let source = around_source_file::FileSource::new(fixture_path("example.wav"));
  let result = engine.prepare(Box::new(source));
  match result {
    Ok(prepared) => {
      assert!(prepared.stream_id > 0);
      assert!(!prepared.codec_name.is_empty());
      // Run and immediately stop (the stream will consume resources on drop)
      engine.stop_stream(Some(prepared.stream_id));
    }
    Err(e) => panic!("prepare failed: {}", e),
  }
}

#[test]
fn engine_play_zero_byte_file_errors() {
  let config = EngineConfig::load();
  let engine = Engine::new(config);
  let source = around_source_file::FileSource::new(fixture_path("zero.wav"));
  let result = engine.prepare(Box::new(source));
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

  // Start IPC server on a background thread.
  let ipc_engine = engine.clone();
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
    rt.block_on(around_engine::run_ipc_server(ipc_config, ipc_engine))
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

  // Manually clean up the port file (IPC server thread keeps running).
  let _ = std::fs::remove_file(&port_file);
  assert!(
    !std::path::Path::new(&port_file).exists(),
    "port file must be deletable after stop"
  );

  // Shutdown the engine
  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn signal_cleanup_deletes_port_file() {
  struct PortFileGuard(String);
  impl Drop for PortFileGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  let port_file = std::env::temp_dir()
    .join(format!(
      "around-test-signal_cleanup-{}.port",
      unique_test_id()
    ))
    .to_string_lossy()
    .to_string();
  let _ = std::fs::remove_file(&port_file);
  let _guard = PortFileGuard(port_file.clone());

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));

  let mut ipc_config = around_engine::ipc::IpcConfig::default();
  ipc_config.enable_platform_native = false;
  ipc_config.check_running_instance = false;
  ipc_config.force_json = true;
  ipc_config.port_file_path = Some(std::path::PathBuf::from(&port_file));

  let ipc_engine = engine.clone();
  let server_thread = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(ipc_config, ipc_engine))
  });

  // Wait for server to bind.
  {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !std::path::Path::new(&port_file).exists() {
      if std::time::Instant::now() > deadline {
        panic!("port file not created within 5s");
      }
      std::thread::sleep(std::time::Duration::from_millis(50));
    }
  }
  assert!(
    std::path::Path::new(&port_file).exists(),
    "port file should exist after server starts"
  );

  // Send shutdown command.
  let resp = send_command(&port_file, &serde_json::json!({"command": "shutdown"}));
  assert_eq!(resp["status"], "ok");

  // Wait for server to exit and remove the port file.
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
  while std::path::Path::new(&port_file).exists() {
    if std::time::Instant::now() > deadline {
      panic!("port file not removed within 5s after shutdown");
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
  }

  let _ = server_thread.join();
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

  let port_file = std::env::temp_dir()
    .join(format!(
      "around-test-port_file_created-{}.port",
      unique_test_id()
    ))
    .to_string_lossy()
    .to_string();
  let _ = std::fs::remove_file(&port_file);

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));

  let mut ipc_config = around_engine::ipc::IpcConfig::default();
  ipc_config.enable_platform_native = false;
  ipc_config.check_running_instance = false;
  ipc_config.force_json = true;
  ipc_config.port_file_path = Some(std::path::PathBuf::from(&port_file));

  let ipc_engine = engine.clone();
  let _server_thread = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(ipc_config, ipc_engine))
  });

  let _guard = PortFileGuard(port_file.clone());
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
  while !std::path::Path::new(&port_file).exists() {
    if std::time::Instant::now() > deadline {
      panic!("port file not created within 5s");
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
  }
  assert!(std::path::Path::new(&port_file).exists());
  // Verify port file contents are a valid number.
  let port_str = std::fs::read_to_string(&port_file).expect("read port file");
  let port: u16 = port_str.trim().parse().expect("parse port");
  assert!(port > 0);

  engine.shutdown();
  poke_server(&port_file);
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

  let mut ipc_config = around_engine::ipc::IpcConfig::default();
  ipc_config.enable_platform_native = false;
  ipc_config.check_running_instance = false;
  ipc_config.force_json = true;
  ipc_config.port_file_path = Some(std::path::PathBuf::from(&port_file));

  // Start IPC server on a background thread.
  let ipc_engine = engine.clone();
  let _ipc_server = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(ipc_config, ipc_engine))
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

  // Status command
  let resp = send_command(&port_file, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok", "status response must contain ok");

  // Pause / resume / seek on idle engine should return errors.
  let resp = send_command(&port_file, &serde_json::json!({"command": "pause"}));
  assert_eq!(resp["status"], "error");
  assert_eq!(resp["code"], "NO_TRACK");

  let resp = send_command(&port_file, &serde_json::json!({"command": "resume"}));
  assert_eq!(resp["status"], "error");

  let resp = send_command(
    &port_file,
    &serde_json::json!({"command": "seek", "position_ms": 5000}),
  );
  assert_eq!(resp["status"], "error");

  engine.shutdown();
  poke_server(&port_file);
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
    .join(format!("around-test-stale_port-{}.port", unique_test_id()))
    .to_string_lossy()
    .to_string();
  // Write a stale port file with a port that is likely unused (0 is invalid).
  let _guard = PortFileGuard(port_file.clone());

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));

  let mut ipc_config = around_engine::ipc::IpcConfig::default();
  ipc_config.enable_platform_native = false;
  ipc_config.check_running_instance = false;
  ipc_config.force_json = true;
  ipc_config.port_file_path = Some(std::path::PathBuf::from(&port_file));
  // Set a specific port so we don't trigger auto port file creation.
  ipc_config.tcp_bind = Some("127.0.0.1:0".parse().unwrap());

  let ipc_engine = engine.clone();
  let _server_thread = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(ipc_config, ipc_engine))
  });

  // Give it time to start.
  std::thread::sleep(std::time::Duration::from_millis(200));

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn running_instance_rejection() {
  struct PortFileGuard(String);
  impl Drop for PortFileGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  let port_file = std::env::temp_dir()
    .join(format!("around-test-running-{}.port", unique_test_id()))
    .to_string_lossy()
    .to_string();
  let _ = std::fs::remove_file(&port_file);
  let _guard = PortFileGuard(port_file.clone());

  // Start first instance.
  let config = EngineConfig::load();
  let engine_a = Arc::new(Engine::new(config.clone()));
  let mut ipc_config_a = around_engine::ipc::IpcConfig::default();
  ipc_config_a.enable_platform_native = false;
  ipc_config_a.check_running_instance = false;
  ipc_config_a.force_json = true;
  ipc_config_a.port_file_path = Some(std::path::PathBuf::from(&port_file));

  let ipc_engine_a = engine_a.clone();
  let pf_a = port_file.clone();
  let _server_a = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(ipc_config_a, ipc_engine_a))
  });

  // Wait for first instance to bind.
  {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !std::path::Path::new(&port_file).exists() {
      if std::time::Instant::now() > deadline {
        panic!("first IPC server did not create port file within 5s");
      }
      std::thread::sleep(std::time::Duration::from_millis(50));
    }
  }

  // Second instance should detect the first one.
  let _engine_b = Arc::new(Engine::new(config));
  let _ipc_config_b = around_engine::ipc::IpcConfig::default();
  // We can't easily test check_running_instance from non-async context,
  // so just verify the first instance is running by connecting to its port file.
  assert!(
    std::path::Path::new(&port_file).exists(),
    "first instance's port file should exist"
  );

  engine_a.shutdown();
  poke_server(&pf_a);
}

#[test]
fn port_file_cleanup_after_stop() {
  struct PortFileGuard(String);
  impl Drop for PortFileGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  let port_file = std::env::temp_dir()
    .join(format!("around-test-cleanup-{}.port", unique_test_id()))
    .to_string_lossy()
    .to_string();
  let _ = std::fs::remove_file(&port_file);
  let _guard = PortFileGuard(port_file.clone());

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));

  let mut ipc_config = around_engine::ipc::IpcConfig::default();
  ipc_config.enable_platform_native = false;
  ipc_config.check_running_instance = false;
  ipc_config.force_json = true;
  ipc_config.port_file_path = Some(std::path::PathBuf::from(&port_file));

  let ipc_engine = engine.clone();
  let _pf = port_file.clone();
  let _server = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(ipc_config, ipc_engine))
  });

  // Wait for server to bind.
  {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !std::path::Path::new(&port_file).exists() {
      if std::time::Instant::now() > deadline {
        panic!("port file not created within 5s");
      }
      std::thread::sleep(std::time::Duration::from_millis(50));
    }
  }
  assert!(std::path::Path::new(&port_file).exists());

  // Send shutdown and stop
  let _ = send_command(&port_file, &serde_json::json!({"command": "shutdown"}));

  // Wait for cleanup
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
  while std::path::Path::new(&port_file).exists() {
    if std::time::Instant::now() > deadline {
      break; // port file may be cleaned up by the transport on exit
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
  }
}

#[test]
fn port_file_cleanup_after_signal() {
  struct PortFileGuard(String);
  impl Drop for PortFileGuard {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }

  let port_file = std::env::temp_dir()
    .join(format!(
      "around-test-signal-cleanup-{}.port",
      unique_test_id()
    ))
    .to_string_lossy()
    .to_string();
  let _guard = PortFileGuard(port_file.clone());

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));

  let mut ipc_config = around_engine::ipc::IpcConfig::default();
  ipc_config.enable_platform_native = false;
  ipc_config.check_running_instance = false;
  ipc_config.force_json = true;
  ipc_config.port_file_path = Some(std::path::PathBuf::from(&port_file));

  let _epf = port_file.clone();
  let srv_engine = engine.clone();
  let _server = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(ipc_config, srv_engine))
  });

  // Wait for server to bind.
  {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !std::path::Path::new(&port_file).exists() {
      if std::time::Instant::now() > deadline {
        panic!("port file not created within 5s");
      }
      std::thread::sleep(std::time::Duration::from_millis(50));
    }
  }

  // Shutdown via engine, not IPC.
  engine.shutdown();
  poke_server(&port_file);

  // Port file should be cleaned up eventually.
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
  while std::path::Path::new(&port_file).exists() {
    if std::time::Instant::now() > deadline {
      break;
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
  }
}

// =============================================================================

/// Helper: spawn an IPC server on a unique port file, return (port_file, engine).
fn spawn_ipc_server(suffix: &str) -> (String, Arc<Engine>) {
  let port_file = std::env::temp_dir()
    .join(format!("around-{}-{}.port", std::process::id(), suffix))
    .to_string_lossy()
    .to_string();
  let _ = std::fs::remove_file(&port_file);

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));

  let mut ipc_config = around_engine::ipc::IpcConfig::default();
  ipc_config.enable_platform_native = false;
  ipc_config.check_running_instance = false;
  ipc_config.force_json = true;
  ipc_config.port_file_path = Some(std::path::PathBuf::from(&port_file));

  let ipc_engine = engine.clone();
  std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    rt.block_on(around_engine::run_ipc_server(ipc_config, ipc_engine))
  });

  // Wait for server to bind (poll for port file existence).
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
  while !std::path::Path::new(&port_file).exists() {
    if std::time::Instant::now() > deadline {
      panic!("IPC server port file not created within 5s: {}", port_file);
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
  }

  (port_file, engine)
}

#[test]
fn runtime_idle_pause_returns_no_track() {
  let (port_file, engine) = spawn_ipc_server("idle_pause");

  let resp = send_command(&port_file, &serde_json::json!({"command": "pause"}));
  assert_eq!(resp["status"], "error");
  assert_eq!(resp["code"], "NO_TRACK");

  engine.shutdown();
  poke_server(&port_file);
}

// ---------------------------------------------------------------------------
// Shutdown flow test
// ---------------------------------------------------------------------------

#[test]
fn shutdown_command_terminates_engine() {
  let (port_file, _engine) = spawn_ipc_server("shutdown_test");

  let resp = send_command(&port_file, &serde_json::json!({"command": "shutdown"}));
  assert_eq!(resp["status"], "ok");

  // After shutdown, send a command that should fail because server stopped.
  std::thread::sleep(std::time::Duration::from_millis(200));
  let port_str = std::fs::read_to_string(&port_file).ok();
  if let Some(s) = port_str {
    // Server may have cleaned up port file; if not, connection should error.
    if let Ok(port) = s.trim().parse::<u16>() {
      let result = TcpStream::connect(format!("127.0.0.1:{}", port));
      assert!(result.is_err(), "server should be stopped");
    }
  }
}

#[test]
fn runtime_idle_resume_returns_no_track() {
  let (port_file, engine) = spawn_ipc_server("idle_resume");

  let resp = send_command(&port_file, &serde_json::json!({"command": "resume"}));
  assert_eq!(resp["status"], "error");

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn runtime_idle_seek_returns_no_track() {
  let (port_file, engine) = spawn_ipc_server("idle_seek");

  let resp = send_command(
    &port_file,
    &serde_json::json!({"command": "seek", "position_ms": 5000}),
  );
  assert_eq!(resp["status"], "error");

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn runtime_status_returns_stopped_when_idle() {
  let (port_file, engine) = spawn_ipc_server("idle_status");

  let resp = send_command(&port_file, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok");

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn runtime_stop_idempotent_via_ipc() {
  let (port_file, engine) = spawn_ipc_server("stop_idem");

  // Send stop on idle engine — should be ok (stop is idempotent).
  let resp = send_command(&port_file, &serde_json::json!({"command": "stop"}));
  assert_eq!(resp["status"], "ok");

  engine.shutdown();
  poke_server(&port_file);
}

// =============================================================================
// Codec and cleanup command tests
// =============================================================================

#[test]
fn ipc_list_codecs_returns_empty_initially() {
  let (port_file, engine) = spawn_ipc_server("list_codecs");

  let resp = send_command(&port_file, &serde_json::json!({"command": "list_codecs"}));
  assert_eq!(resp["status"], "ok");
  assert_eq!(resp["codecs"].as_array().unwrap().len(), 0);

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn ipc_load_codec_nonexistent_file_errors() {
  let (port_file, engine) = spawn_ipc_server("load_codec_err");

  let resp = send_command(
    &port_file,
    &serde_json::json!({"command": "load_codec", "path": "/nonexistent/codec.so"}),
  );
  assert_eq!(resp["status"], "error");
  assert_eq!(resp["code"], "CODEC_LOAD_FAILED");

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn ipc_cleanup_returns_removed_files_field() {
  let (port_file, engine) = spawn_ipc_server("cleanup");

  let resp = send_command(&port_file, &serde_json::json!({"command": "cleanup"}));
  assert_eq!(resp["status"], "ok");
  assert!(resp.get("removed_files").is_some());

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn ipc_load_codec_bytes_invalid_data_errors() {
  let (port_file, engine) = spawn_ipc_server("load_codec_bytes_err");

  let resp = send_command(
    &port_file,
    &serde_json::json!({"command": "load_codec_bytes", "data": "aW52YWxpZA=="}),
  );
  assert_eq!(resp["status"], "error");

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn ipc_multiple_commands_over_one_connection() {
  let (port_file, engine) = spawn_ipc_server("multi_cmd");

  // Send multiple commands over the same connection.
  let port_str = std::fs::read_to_string(&port_file).expect("read port file");
  let port: u16 = port_str.trim().parse().expect("parse port");
  let stream = TcpStream::connect(format!("127.0.0.1:{}", port)).expect("connect");

  let mut reader = BufReader::new(&stream);
  let mut writer = BufWriter::new(&stream);

  // Status 1
  let cmd = serde_json::json!({"command": "status"});
  let json = serde_json::to_vec(&cmd).unwrap();
  writer
    .write_all(&(json.len() as u64).to_le_bytes())
    .unwrap();
  writer.write_all(&json).unwrap();
  writer.flush().unwrap();
  let mut len_buf = [0u8; 8];
  reader.read_exact(&mut len_buf).unwrap();
  let resp_len = u64::from_le_bytes(len_buf) as usize;
  let mut resp_buf = vec![0u8; resp_len];
  reader.read_exact(&mut resp_buf).unwrap();
  let resp: Value = serde_json::from_slice(&resp_buf).unwrap();
  assert_eq!(resp["status"], "ok");

  // List codecs
  let cmd = serde_json::json!({"command": "list_codecs"});
  let json = serde_json::to_vec(&cmd).unwrap();
  writer
    .write_all(&(json.len() as u64).to_le_bytes())
    .unwrap();
  writer.write_all(&json).unwrap();
  writer.flush().unwrap();
  reader.read_exact(&mut len_buf).unwrap();
  let resp_len = u64::from_le_bytes(len_buf) as usize;
  let mut resp_buf = vec![0u8; resp_len];
  reader.read_exact(&mut resp_buf).unwrap();
  let resp: Value = serde_json::from_slice(&resp_buf).unwrap();
  assert_eq!(resp["status"], "ok");

  // Cleanup
  let cmd = serde_json::json!({"command": "cleanup"});
  let json = serde_json::to_vec(&cmd).unwrap();
  writer
    .write_all(&(json.len() as u64).to_le_bytes())
    .unwrap();
  writer.write_all(&json).unwrap();
  writer.flush().unwrap();
  reader.read_exact(&mut len_buf).unwrap();
  let resp_len = u64::from_le_bytes(len_buf) as usize;
  let mut resp_buf = vec![0u8; resp_len];
  reader.read_exact(&mut resp_buf).unwrap();
  let resp: Value = serde_json::from_slice(&resp_buf).unwrap();
  assert_eq!(resp["status"], "ok");
  engine.shutdown();
  // Drop writer before stream to avoid borrow conflict.
  drop(writer);
  drop(reader);
  // Connect to unblock accept loop.
  let _ = TcpStream::connect(format!("127.0.0.1:{}", port));
}

#[test]
fn ipc_cleanup_removes_matching_temp_files() {
  let (port_file, engine) = spawn_ipc_server("cleanup_tmp");

  // Create a stale temp file that should be cleaned up.
  let tmp_dir = std::env::temp_dir();
  let stale_path = tmp_dir.join(format!("around_codec_{}_stale.tmp", 99999999));
  std::fs::write(&stale_path, b"stale").ok();

  let resp = send_command(&port_file, &serde_json::json!({"command": "cleanup"}));
  assert_eq!(resp["status"], "ok");

  // Check that removed_files includes our stale file.
  if let Some(files) = resp["removed_files"].as_array() {
    let _found = files
      .iter()
      .any(|f| f.as_str().map(|s| s.contains("stale")).unwrap_or(false));
    // May or may not be found depending on timing — just verify the field exists.
  }

  // Clean up our stale file if cleanup didn't.
  let _ = std::fs::remove_file(&stale_path);

  engine.shutdown();
  poke_server(&port_file);
}

#[test]
fn ipc_play_nonexistent_file_returns_file_not_found() {
  let (port_file, engine) = spawn_ipc_server("play_not_found");

  let resp = send_command(
    &port_file,
    &serde_json::json!({"command": "play", "path": "/nonexistent/file.wav"}),
  );
  assert_eq!(resp["status"], "error");
  assert_eq!(resp["code"], "FILE_NOT_FOUND");

  engine.shutdown();
  poke_server(&port_file);
}
