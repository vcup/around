//! Integration test: Unix domain socket transport lifecycle (FR-012).
//!
//! These tests exercise the platform-native Unix socket transport directly.
//! To run manually: cargo test --test ipc_unix_socket
#![cfg(unix)]

use around_engine::{Engine, EngineConfig, PlaybackState};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

fn socket_path() -> std::path::PathBuf {
  if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
    return std::path::PathBuf::from(&dir)
      .join("around")
      .join("around.sock");
  }
  if let Ok(dir) = std::env::var("TMPDIR") {
    return std::path::PathBuf::from(&dir).join("around.sock");
  }
  std::env::temp_dir().join("around.sock")
}

fn test_socket_path(suffix: &str) -> std::path::PathBuf {
  std::env::temp_dir().join(format!(
    "around-test-{}-{}.sock",
    std::process::id(),
    suffix
  ))
}

fn send_unix_command(socket: &std::path::Path, cmd: &serde_json::Value) -> serde_json::Value {
  for attempt in 0..50 {
    match UnixStream::connect(socket) {
      Ok(mut stream) => {
        let req = cmd.to_string() + "\n";
        stream.write_all(req.as_bytes()).expect("write command");
        stream
          .shutdown(std::net::Shutdown::Write)
          .expect("shutdown write");
        let mut buf = String::new();
        let mut reader = BufReader::new(&stream);
        reader.read_line(&mut buf).expect("read response");
        return serde_json::from_str(buf.trim()).expect("parse response");
      }
      Err(_) if attempt < 49 => {
        std::thread::sleep(std::time::Duration::from_millis(100));
      }
      Err(e) => panic!("connect failed after {} attempts: {}", attempt + 1, e),
    }
  }
  unreachable!()
}

struct SocketGuard(std::path::PathBuf);
impl Drop for SocketGuard {
  fn drop(&mut self) {
    let _ = std::fs::remove_file(&self.0);
  }
}

fn spawn_server(
  engine: Arc<Engine>,
  state: Arc<Mutex<PlaybackState>>,
  socket_path: std::path::PathBuf,
) -> std::thread::JoinHandle<Result<(), Box<dyn std::error::Error + Send + Sync>>> {
  std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    let mut ipc_config = around_engine::ipc::IpcConfig::default();
    let port_file_name = format!(
      "around-test-unix-{}.port",
      socket_path.file_name().unwrap().to_string_lossy()
    );
    ipc_config.port_file_path = Some(std::env::temp_dir().join(port_file_name));
    ipc_config.unix_socket_path = Some(socket_path);
    ipc_config.check_running_instance = false;
    ipc_config.force_json = true;
    rt.block_on(around_engine::run_ipc_server(ipc_config, engine, state))
  })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn unix_socket_created_with_correct_permissions() {
  let sp = test_socket_path("perms");
  let _guard = SocketGuard(sp.clone());
  let _ = std::fs::remove_file(&sp);

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  let sp_clone = sp.clone();
  let server = spawn_server(engine.clone(), state.clone(), sp.clone());

  use std::os::unix::fs::PermissionsExt;
  let resp = send_unix_command(&sp, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok");

  // Check permissions once socket is created (send_unix_command ensures it's ready).
  let meta = std::fs::metadata(&sp).expect("metadata");
  let mode = meta.permissions().mode();
  assert_eq!(
    mode & 0o777,
    0o600,
    "socket must have 0600 permissions, got {:o}",
    mode & 0o777
  );

  engine.shutdown();
  // Unblock server's accept loop so join can complete.
  let _ = UnixStream::connect(&sp_clone);
  let _ = server.join();
}

#[test]
fn resolve_socket_dir_falls_back_to_tmpdir() {
  // socket_path() resolves to XDG_RUNTIME_DIR → TMPDIR → temp_dir().
  // The directory is created lazily by bind_unix_socket(), not by socket_path().
  // Verify the resolved path is under a standard base directory.
  let sp = socket_path();
  let tmp = std::env::temp_dir();
  assert!(
    sp.starts_with(&tmp) || std::env::var("XDG_RUNTIME_DIR").is_ok(),
    "socket_path must be under temp_dir unless XDG_RUNTIME_DIR is set"
  );
  // socket_path() returns a path ending with "around.sock".
  assert!(sp.ends_with("around.sock"));
}

#[test]
fn unix_socket_status_command_round_trip() {
  let sp = test_socket_path("roundtrip");
  let _guard = SocketGuard(sp.clone());
  let _ = std::fs::remove_file(&sp);

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  let sp_clone = sp.clone();
  let server = spawn_server(engine.clone(), state.clone(), sp.clone());

  let resp = send_unix_command(&sp, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok");
  assert_eq!(resp["state"], "stopped", "idle state must be stopped");

  engine.shutdown();
  // Unblock server's accept loop so join can complete.
  let _ = UnixStream::connect(&sp_clone);
  let _ = server.join();
}

#[test]
fn duplicate_instance_rejected_via_unix_socket() {
  let sp = test_socket_path("dup");
  let _guard = SocketGuard(sp.clone());
  let _ = std::fs::remove_file(&sp);

  let config = EngineConfig::load();
  let engine1 = Arc::new(Engine::new(config.clone()));
  let state1 = Arc::new(Mutex::new(PlaybackState::default()));

  let sp_clone = sp.clone();
  let server = spawn_server(engine1.clone(), state1.clone(), sp.clone());

  // Ensure first instance is listening before attempting duplicate.
  let _ = send_unix_command(&sp, &serde_json::json!({"command": "status"}));

  let engine2 = Arc::new(Engine::new(config));
  let state2 = Arc::new(Mutex::new(PlaybackState::default()));
  let rt = tokio::runtime::Builder::new_current_thread()
    .enable_io()
    .enable_time()
    .build()
    .expect("tokio runtime");
  let mut ipc_config2 = around_engine::ipc::IpcConfig::default();
  ipc_config2.unix_socket_path = Some(sp.clone());
  let result = rt.block_on(around_engine::run_ipc_server(ipc_config2, engine2, state2));
  assert!(result.is_err(), "second instance must be rejected");
  let err_msg = result.unwrap_err().to_string();
  assert!(
    err_msg.contains("already running"),
    "error must mention already running, got: {}",
    err_msg
  );

  engine1.shutdown();
  // Unblock server's accept loop so join can complete.
  let _ = UnixStream::connect(&sp_clone);
  let _ = server.join();
}

#[test]
fn stale_unix_socket_detected_and_replaced() {
  // US1-4: Stale socket file from crashed process is detected and replaced.
  let sp = test_socket_path("stale");
  let _guard = SocketGuard(sp.clone());
  let _ = std::fs::remove_file(&sp);

  // Create a stale socket file by binding and immediately dropping a UnixListener.
  {
    use std::os::unix::net::UnixListener;
    let _listener = UnixListener::bind(&sp).expect("bind stale socket");
    // Listener dropped here — socket file remains but no one is listening.
  }
  assert!(
    sp.exists(),
    "stale socket file must exist before engine starts"
  );

  // Engine should detect the stale socket, remove it, and bind fresh.
  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  let sp_clone = sp.clone();
  let server = spawn_server(engine.clone(), state.clone(), sp.clone());

  // send_unix_command retries connect, so we don't need a separate wait.
  let resp = send_unix_command(&sp, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok");

  engine.shutdown();
  // Unblock server's accept loop so join can complete.
  let _ = UnixStream::connect(&sp_clone);
  let _ = server.join();
}
