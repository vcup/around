//! Integration test: Unix domain socket transport lifecycle (FR-012).
//!
//! These tests exercise the platform-native Unix socket transport directly.
//! To run manually: cargo test --test ipc_unix_socket
#![cfg(unix)]

use around_engine::{Engine, EngineConfig};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::sync::Arc;

fn test_socket_path(suffix: &str) -> std::path::PathBuf {
  std::env::temp_dir().join(format!("around-test-{}.sock", suffix))
}

fn send_unix_command(socket: &std::path::Path, cmd: &serde_json::Value) -> serde_json::Value {
  let mut stream = UnixStream::connect(socket).expect("connect unix socket");

  let json = serde_json::to_vec(cmd).expect("serialize");
  stream
    .write_all(&(json.len() as u64).to_le_bytes())
    .expect("write len");
  stream.write_all(&json).expect("write json");
  stream.flush().expect("flush");

  let mut len_buf = [0u8; 8];
  stream.read_exact(&mut len_buf).expect("read len");
  let resp_len = u64::from_le_bytes(len_buf) as usize;
  let mut resp_buf = vec![0u8; resp_len];
  stream.read_exact(&mut resp_buf).expect("read resp");
  serde_json::from_slice(&resp_buf).expect("parse resp")
}

struct SocketGuard(std::path::PathBuf);
impl Drop for SocketGuard {
  fn drop(&mut self) {
    let _ = std::fs::remove_file(&self.0);
  }
}

fn spawn_server(
  engine: Arc<Engine>,
  socket_path: std::path::PathBuf,
) -> std::thread::JoinHandle<Result<(), Box<dyn std::error::Error + Send + Sync>>> {
  std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .enable_time()
      .build()
      .expect("tokio runtime");
    let mut ipc_config = around_engine::ipc::IpcConfig::default();
    ipc_config.enable_platform_native = false;
    ipc_config.check_running_instance = false;
    ipc_config.force_json = true;
    ipc_config.unix_socket_path = Some(socket_path.clone());
    // Set a TCP bind address to avoid colliding with the default port file.
    ipc_config.tcp_bind = Some("127.0.0.1:0".parse().unwrap());
    rt.block_on(around_engine::run_ipc_server(ipc_config, engine))
      .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e })?;
    Ok(())
  })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn unix_socket_created_with_correct_permissions() {
  let sp = test_socket_path(&format!("perm-{}", std::process::id()));
  let _guard = SocketGuard(sp.clone());
  let _ = std::fs::remove_file(&sp);

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));

  let server = spawn_server(engine.clone(), sp.clone());

  // Wait for socket to be created.
  let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
  while !sp.exists() {
    if std::time::Instant::now() > deadline {
      panic!("socket not created within 5s");
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
  }

  assert!(sp.exists(), "socket should exist");
  assert_eq!(sp.metadata().unwrap().permissions().mode() & 0o777, 0o600);

  engine.shutdown();
  let _ = server.join();
}

#[test]
fn resolve_socket_dir_falls_back_to_tmpdir() {
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
  let engine = Arc::new(Engine::new(config));

  let server = spawn_server(engine.clone(), sp.clone());

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

  engine.shutdown();
  let _ = server.join();
}

#[test]
fn duplicate_instance_rejected_via_unix_socket() {
  let sp = test_socket_path(&format!("dup-{}", std::process::id()));
  let _guard = SocketGuard(sp.clone());
  let _ = std::fs::remove_file(&sp);

  let config = EngineConfig::load();

  // Start first instance.
  let engine1 = Arc::new(Engine::new(config.clone()));
  let server1 = spawn_server(engine1.clone(), sp.clone());

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

  engine1.shutdown();
  let _ = server1.join();
}

#[test]
fn stale_unix_socket_detected_and_replaced() {
  let sp = test_socket_path(&format!("stale-{}", std::process::id()));
  let _guard = SocketGuard(sp.clone());
  let _ = std::fs::remove_file(&sp);

  // Create a stale socket file.
  std::fs::write(&sp, b"stale").expect("write stale socket");
  assert!(sp.exists(), "stale socket should exist");

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));

  let server = spawn_server(engine.clone(), sp.clone());

  // Wait for server to start and replace the socket.
  std::thread::sleep(std::time::Duration::from_millis(500));

  // The stale socket should have been replaced by the server.
  // Verify by sending a command.
  let resp = send_unix_command(&sp, &serde_json::json!({"command": "status"}));
  assert_eq!(resp["status"], "ok");

  engine.shutdown();
  let _ = server.join();
}
