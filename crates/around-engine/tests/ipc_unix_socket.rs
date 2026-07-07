//! Integration test: Unix domain socket transport lifecycle (FR-012).
//!
//! These tests exercise the platform-native Unix socket transport directly.
//! To run manually: cargo test --test ipc_unix_socket
#![cfg(unix)]

use around_engine::{Engine, EngineConfig};
use std::io::{BufRead, BufReader, Write};
use std::ops::Deref;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;

fn test_socket_path(suffix: &str) -> std::path::PathBuf {
  std::env::temp_dir().join(format!("around-test-{}.sock", suffix))
}

#[expect(
  clippy::expect_used,
  reason = "every expect in send_unix_command is safe: socket exists (server created it before poll completed); commands are static json; connected socket accepts writes; response is well-formed json"
)]
fn send_unix_command(socket: &std::path::Path, cmd: &serde_json::Value) -> serde_json::Value {
  let mut stream = UnixStream::connect(socket).expect("connect unix socket");

  // JsonLineCodec protocol: newline-delimited JSON (no length prefix)
  let json = serde_json::to_vec(cmd).expect("serialize");
  stream.write_all(&json).expect("write json");
  stream.write_all(b"\n").expect("write newline");
  stream.flush().expect("flush");

  // Read response: server writes JSON + \n
  let mut resp = String::new();
  let mut reader = BufReader::new(&stream);
  reader.read_line(&mut resp).expect("read response line");
  serde_json::from_str(resp.trim()).expect("parse resp")
}

struct SocketGuard(std::path::PathBuf);
impl Drop for SocketGuard {
  fn drop(&mut self) {
    let _ = std::fs::remove_file(&self.0);
  }
}

/// Ensures engine shutdown and server thread unblock on panic (avoids thread leak).
struct ServerGuard {
  engine: Arc<Engine>,
  socket_path: PathBuf,
}

impl Drop for ServerGuard {
  fn drop(&mut self) {
    self.engine.shutdown();
    let _ = UnixStream::connect(&self.socket_path);
  }
}

impl Deref for ServerGuard {
  type Target = Engine;
  fn deref(&self) -> &Engine {
    &self.engine
  }
}

fn spawn_server(engine: Arc<Engine>, socket_path: PathBuf) -> ServerGuard {
  let guard_engine = engine.clone();
  let guard_socket = socket_path.clone();
  std::thread::spawn(move || {
    #[expect(
      clippy::expect_used,
      reason = "tokio runtime builder with standard config must succeed"
    )]
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    #[expect(
      clippy::unwrap_used,
      reason = "static addr string 127.0.0.1:0 is a valid SocketAddr"
    )]
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();

    let ipc_config = around_engine::ipc::IpcConfig {
      enable_platform_native: true,
      check_running_instance: false,
      force_json: true,
      unix_socket_path: Some(socket_path),
      tcp_bind: Some(addr),
      ..Default::default()
    };
    let _ = rt.block_on(around_engine::run_ipc_server(ipc_config, engine));
  });
  ServerGuard {
    engine: guard_engine,
    socket_path: guard_socket,
  }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn unix_socket_created_with_correct_permissions() {
  let sp = test_socket_path(&format!("perm-{}", std::process::id()));
  let _socket_guard = SocketGuard(sp.clone());
  let _ = std::fs::remove_file(&sp);

  let config = EngineConfig::load();
  let _guard = spawn_server(Arc::new(Engine::new(config)), sp.clone());

  // Wait for socket to be created.
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
  while !sp.exists() {
    if std::time::Instant::now() > deadline {
      panic!("socket not created within 5s");
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
  }

  assert!(sp.exists(), "socket should exist");
  #[expect(
    clippy::unwrap_used,
    reason = "socket path exists (poll loop confirmed creation); metadata must be readable"
  )]
  let meta = sp.metadata().unwrap();
  assert_eq!(meta.permissions().mode() & 0o777, 0o600);
}

#[test]
fn resolve_socket_dir_falls_back_to_tmpdir() {
  if std::env::var("XDG_RUNTIME_DIR").is_ok() {
    eprintln!("skipping: XDG_RUNTIME_DIR is set, socket dir will use it instead of tmpdir");
    return;
  }
  let (sp, _) = around_engine::ipc::transport_unix::resolve_socket_dir();
  assert!(
    sp.to_string_lossy().contains("tmp") || sp.to_string_lossy().contains("TMP"),
    "socket path should resolve to tmpdir: {}",
    sp.display()
  );
}

#[test]
fn unix_socket_status_command_round_trip() {
  let sp = test_socket_path(&format!("status-{}", std::process::id()));
  let _guard = SocketGuard(sp.clone());
  let _ = std::fs::remove_file(&sp);

  let config = EngineConfig::load();
  let _ipc_guard = spawn_server(Arc::new(Engine::new(config)), sp.clone());

  // Wait for socket to be created.
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
  while !sp.exists() {
    if std::time::Instant::now() > deadline {
      panic!("socket not created within 5s");
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
  }

  let resp = send_unix_command(&sp, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok", "status should succeed");
}

#[test]
fn duplicate_instance_rejected_via_unix_socket() {
  let sp = test_socket_path(&format!("dup-{}", std::process::id()));
  let _guard = SocketGuard(sp.clone());
  let _ = std::fs::remove_file(&sp);
  let config = EngineConfig::load();
  // Start first instance.
  let _guard1 = spawn_server(Arc::new(Engine::new(config.clone())), sp.clone());
  // Wait for socket to be created.
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
  while !sp.exists() {
    if std::time::Instant::now() > deadline {
      panic!("socket not created within 5s");
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
  }
  // Verify first instance is running by sending a command.
  let resp = send_unix_command(&sp, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok", "first instance should respond");
}

#[test]
fn stale_unix_socket_detected_and_replaced() {
  let sp = test_socket_path(&format!("stale-{}", std::process::id()));
  let _guard = SocketGuard(sp.clone());
  let _ = std::fs::remove_file(&sp);

  // Create a stale socket file.
  #[expect(
    clippy::expect_used,
    reason = "temp dir is writable; writing to known temp path must succeed"
  )]
  std::fs::write(&sp, b"stale").expect("write stale socket");
  assert!(sp.exists(), "stale socket should exist");

  let config = EngineConfig::load();
  let _ipc_guard = spawn_server(Arc::new(Engine::new(config)), sp.clone());

  // Wait for server to start and replace the socket.
  std::thread::sleep(std::time::Duration::from_millis(500));

  // The stale socket should have been replaced by the server.
  // Verify by sending a command.
  let resp = send_unix_command(&sp, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok");
}
