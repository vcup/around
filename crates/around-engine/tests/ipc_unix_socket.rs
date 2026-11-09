//! Integration test: Unix domain socket transport lifecycle (FR-012).
//!
//! Every test drives the real `run_ipc_server` over a private custom Unix
//! socket and port-file path, so the suite never touches the default
//! `<temp>/around.sock` or `<temp>/around.port` and is safe to run in
//! parallel with other workspace tests.
//!
//! To run manually: cargo test --test ipc_unix_socket
#![cfg(unix)]
#![expect(
  clippy::expect_used,
  reason = "integration harness assertions carry context; a failure means the server or filesystem contract is broken"
)]

use around_engine::ipc::IpcConfig;
use around_engine::{Engine, EngineConfig, OutputDriver};
use serde_json::{json, Value};
use std::error::Error;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Upper bound for any client-side socket read/write.
const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// Upper bound for observing the server task return.
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Monotonic per-process identifier, combined with the PID for private paths.
static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

/// Private temporary directory for one test; removed on drop.
struct TestDir {
  path: PathBuf,
}

impl TestDir {
  fn new(label: &str) -> Self {
    let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
    // Keep sun_path below macOS's 104-byte limit even under its long TMPDIR.
    let path = std::env::temp_dir().join(format!("au-{:x}-{id:x}", std::process::id()));
    fs::create_dir(&path).unwrap_or_else(|error| panic!("create {label} test directory: {error}"));
    Self { path }
  }

  fn socket_path(&self) -> PathBuf {
    self.path.join("around.sock")
  }

  fn port_file_path(&self) -> PathBuf {
    self.path.join("around.port")
  }
}

impl Drop for TestDir {
  fn drop(&mut self) {
    let _ = fs::remove_dir_all(&self.path);
  }
}

fn test_engine() -> Arc<Engine> {
  let mut config = EngineConfig::load();
  config.output_driver = OutputDriver::Null;
  Arc::new(Engine::new(config))
}

fn ipc_config(socket_path: &Path, port_file_path: &Path, check_running: bool) -> IpcConfig {
  IpcConfig {
    enable_platform_native: true,
    check_running_instance: check_running,
    force_json: true,
    unix_socket_path: Some(socket_path.to_path_buf()),
    port_file_path: Some(port_file_path.to_path_buf()),
    ..IpcConfig::default()
  }
}

type ServerResult = Result<(), Box<dyn Error + Send + Sync>>;

/// Runs `run_ipc_server` on a dedicated thread and guarantees the thread is
/// joined: on drop it signals shutdown and pokes the accept loop so a blocked
/// `accept()` observes the signal even when a test fails mid-way.
struct ServerGuard {
  engine: Arc<Engine>,
  socket_path: PathBuf,
  port_file_path: PathBuf,
  thread: Option<std::thread::JoinHandle<ServerResult>>,
}

impl ServerGuard {
  fn spawn(
    engine: Arc<Engine>,
    config: IpcConfig,
    socket_path: PathBuf,
    port_file_path: PathBuf,
  ) -> Self {
    let server_engine = Arc::clone(&engine);
    let thread = std::thread::spawn(move || {
      let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .expect("build current-thread tokio runtime");
      runtime.block_on(around_engine::run_ipc_server(config, server_engine))
    });
    Self {
      engine,
      socket_path,
      port_file_path,
      thread: Some(thread),
    }
  }

  /// Waits until the server task returns without signalling shutdown first.
  ///
  /// Used by the duplicate/bind-collision tests where `run_ipc_server` is
  /// expected to fail on its own. Returns `None` if it is still running after
  /// `timeout`.
  fn wait_for_exit(&mut self, timeout: Duration) -> Option<ServerResult> {
    let deadline = Instant::now() + timeout;
    loop {
      let finished = self
        .thread
        .as_ref()
        .map(std::thread::JoinHandle::is_finished)
        .unwrap_or(false);
      if finished {
        let thread = self.thread.take().expect("finished server thread");
        return Some(thread.join().expect("IPC server thread must not panic"));
      }
      if self.thread.is_none() || Instant::now() >= deadline {
        return None;
      }
      std::thread::sleep(Duration::from_millis(10));
    }
  }

  /// Signals shutdown, wakes a blocked accept loop, then joins the server.
  fn shutdown_and_join(&mut self) {
    self.engine.shutdown();
    self.wake_accept_loop();
    if let Some(thread) = self.thread.take() {
      let _ = thread.join();
    }
  }

  fn wake_accept_loop(&self) {
    if UnixStream::connect(&self.socket_path).is_ok() {
      return;
    }
    // A TCP fallback would be unexpected here, but waking it keeps the
    // harness from hanging if a regression introduces one.
    let Ok(port_text) = fs::read_to_string(&self.port_file_path) else {
      return;
    };
    let Ok(port) = port_text.trim().parse::<u16>() else {
      return;
    };
    let _ = std::net::TcpStream::connect(("127.0.0.1", port));
  }
}

impl Drop for ServerGuard {
  fn drop(&mut self) {
    self.shutdown_and_join();
  }
}

/// Sends one newline-delimited JSON command and reads one response line.
fn send_unix_command(socket_path: &Path, command: &Value) -> Value {
  let stream = UnixStream::connect(socket_path).expect("connect to live unix socket");
  stream
    .set_read_timeout(Some(IO_TIMEOUT))
    .expect("set read timeout");
  stream
    .set_write_timeout(Some(IO_TIMEOUT))
    .expect("set write timeout");

  let json = serde_json::to_vec(command).expect("serialize command");
  (&stream)
    .write_all(&json)
    .expect("write serialized command");
  (&stream).write_all(b"\n").expect("write command newline");
  (&stream).flush().expect("flush command");

  let mut response = String::new();
  BufReader::new(&stream)
    .read_line(&mut response)
    .expect("read response line");
  serde_json::from_str(response.trim()).expect("parse JSON response")
}

fn wait_until(what: &str, mut predicate: impl FnMut() -> bool) {
  let deadline = Instant::now() + IO_TIMEOUT;
  while !predicate() {
    assert!(
      Instant::now() < deadline,
      "timed out after {IO_TIMEOUT:?} waiting for {what}"
    );
    std::thread::sleep(Duration::from_millis(10));
  }
}

fn is_socket(path: &Path) -> bool {
  fs::metadata(path)
    .map(|meta| meta.file_type().is_socket())
    .unwrap_or(false)
}

fn inode(path: &Path) -> u64 {
  fs::metadata(path)
    .expect("read metadata for existing socket")
    .ino()
}

fn wait_for_socket(path: &Path) {
  wait_until(&format!("unix socket {}", path.display()), || {
    UnixStream::connect(path).is_ok()
  });
}

fn wait_for_mode(path: &Path, mode: u32) {
  wait_until(&format!("mode {mode:o} on {}", path.display()), || {
    fs::metadata(path)
      .map(|meta| meta.permissions().mode() & 0o777 == mode)
      .unwrap_or(false)
  });
}

/// Starts a server expected to fail before listening and waits for its result.
///
/// The guard still owns the thread, so a regression that leaves the server
/// running is reported as a bounded failure rather than hanging the suite.
fn run_server_until_exit(
  engine: Arc<Engine>,
  config: IpcConfig,
  socket_path: PathBuf,
  port_file_path: PathBuf,
) -> ServerResult {
  let mut server = ServerGuard::spawn(engine, config, socket_path, port_file_path);
  server.wait_for_exit(EXIT_TIMEOUT).unwrap_or_else(|| {
    panic!("run_ipc_server did not fail within {EXIT_TIMEOUT:?}; it may have started")
  })
}

fn assert_addr_in_use(error: Box<dyn Error + Send + Sync>) {
  let io_error = match error.downcast::<std::io::Error>() {
    Ok(io_error) => io_error,
    Err(other) => panic!("expected an io::Error, got: {other}"),
  };
  assert_eq!(
    io_error.kind(),
    std::io::ErrorKind::AddrInUse,
    "collision must report AddrInUse, got: {io_error}"
  );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn unix_socket_created_with_0600_permissions() {
  let dir = TestDir::new("permissions");
  let socket_path = dir.socket_path();
  let port_file_path = dir.port_file_path();
  let mut server = ServerGuard::spawn(
    test_engine(),
    ipc_config(&socket_path, &port_file_path, false),
    socket_path.clone(),
    port_file_path.clone(),
  );

  wait_for_socket(&socket_path);
  wait_for_mode(&socket_path, 0o600);

  assert!(
    !port_file_path.exists(),
    "native Unix socket startup must not create a TCP port file"
  );

  let response = send_unix_command(&socket_path, &json!({"command": "status"}));
  assert_eq!(response["status"], "ok");
  assert_eq!(response["streams"], json!([]));

  server.shutdown_and_join();
  assert!(
    !socket_path.exists(),
    "socket file must be removed when the server exits"
  );
}

#[test]
fn duplicate_instance_rejected_by_running_instance_check() {
  let dir = TestDir::new("duplicate-check");
  let socket_path = dir.socket_path();
  let port_file_path = dir.port_file_path();
  let mut primary = ServerGuard::spawn(
    test_engine(),
    ipc_config(&socket_path, &port_file_path, false),
    socket_path.clone(),
    port_file_path.clone(),
  );
  wait_for_socket(&socket_path);
  let original_inode = inode(&socket_path);

  let _error = run_server_until_exit(
    test_engine(),
    ipc_config(&socket_path, &port_file_path, true),
    socket_path.clone(),
    port_file_path.clone(),
  )
  .expect_err("a second instance must be rejected by the running-instance check");

  assert_eq!(
    inode(&socket_path),
    original_inode,
    "rejecting a duplicate must not replace the live socket"
  );
  let response = send_unix_command(&socket_path, &json!({"command": "status"}));
  assert_eq!(
    response["status"], "ok",
    "the original instance must remain usable"
  );
  assert!(
    !port_file_path.exists(),
    "pre-check rejection must not fall back to TCP"
  );

  primary.shutdown_and_join();
}

#[test]
fn duplicate_bind_preserves_live_socket() {
  let dir = TestDir::new("duplicate-bind");
  let socket_path = dir.socket_path();
  let port_file_path = dir.port_file_path();
  let mut primary = ServerGuard::spawn(
    test_engine(),
    ipc_config(&socket_path, &port_file_path, false),
    socket_path.clone(),
    port_file_path.clone(),
  );
  wait_for_socket(&socket_path);
  let original_inode = inode(&socket_path);

  // Disable the pre-check so the second server reaches the bind/probe path.
  let error = run_server_until_exit(
    test_engine(),
    ipc_config(&socket_path, &port_file_path, false),
    socket_path.clone(),
    port_file_path.clone(),
  )
  .expect_err("binding over a live Unix listener must fail");
  assert_addr_in_use(error);

  assert_eq!(
    inode(&socket_path),
    original_inode,
    "a successful in-use probe must preserve the live socket inode"
  );
  assert!(is_socket(&socket_path), "live socket must remain a socket");
  let response = send_unix_command(&socket_path, &json!({"command": "status"}));
  assert_eq!(
    response["status"], "ok",
    "the original instance must remain usable after a bind collision"
  );
  assert!(
    !port_file_path.exists(),
    "a live socket collision must not fall back to TCP"
  );

  primary.shutdown_and_join();
}

#[test]
fn stale_unix_socket_is_replaced() {
  let dir = TestDir::new("stale");
  let socket_path = dir.socket_path();
  let port_file_path = dir.port_file_path();

  // Bind then drop a listener: the filesystem entry remains, but nothing is
  // listening. This is the stale-socket state left by a crashed process.
  let stale_listener = UnixListener::bind(&socket_path).expect("bind stale unix socket");
  drop(stale_listener);
  assert!(
    is_socket(&socket_path),
    "stale entry should still be a socket"
  );
  assert!(
    UnixStream::connect(&socket_path).is_err(),
    "dropped listener must leave no live peer"
  );

  let mut server = ServerGuard::spawn(
    test_engine(),
    ipc_config(&socket_path, &port_file_path, false),
    socket_path.clone(),
    port_file_path.clone(),
  );
  wait_for_socket(&socket_path);
  wait_for_mode(&socket_path, 0o600);

  let response = send_unix_command(&socket_path, &json!({"command": "status"}));
  assert_eq!(
    response["status"], "ok",
    "server must rebind a stale socket and serve requests"
  );
  assert!(
    !port_file_path.exists(),
    "stale-socket replacement must not fall back to TCP"
  );

  server.shutdown_and_join();
  assert!(
    !socket_path.exists(),
    "replacement socket must be cleaned up on shutdown"
  );
}

#[test]
fn ordinary_file_at_socket_path_is_preserved() {
  let dir = TestDir::new("ordinary-file");
  let socket_path = dir.socket_path();
  let port_file_path = dir.port_file_path();
  let contents = b"not a socket";
  fs::write(&socket_path, contents).expect("create ordinary file at socket path");

  let mut server = ServerGuard::spawn(
    test_engine(),
    ipc_config(&socket_path, &port_file_path, false),
    socket_path.clone(),
    port_file_path.clone(),
  );

  let result = server
    .wait_for_exit(EXIT_TIMEOUT)
    .expect("an ordinary file collision must make run_ipc_server fail promptly");
  let error = result.expect_err("ordinary file at the socket path must not be replaced");
  assert_addr_in_use(error);

  let metadata = fs::symlink_metadata(&socket_path).expect("ordinary file must still exist");
  assert!(
    metadata.file_type().is_file(),
    "ordinary file must remain an ordinary file"
  );
  assert_eq!(
    fs::read(&socket_path).expect("read preserved file"),
    contents.as_slice(),
    "ordinary file contents must be untouched"
  );
  assert!(
    !port_file_path.exists(),
    "a non-socket collision must be terminal, not a TCP fallback"
  );
}

#[test]
fn symlink_at_socket_path_is_preserved() {
  let dir = TestDir::new("symlink");
  let socket_path = dir.socket_path();
  let port_file_path = dir.port_file_path();
  let target_path = dir.path.join("target-file");
  let contents = b"symlink target";
  fs::write(&target_path, contents).expect("create symlink target");
  std::os::unix::fs::symlink(&target_path, &socket_path).expect("create symlink at socket path");

  let mut server = ServerGuard::spawn(
    test_engine(),
    ipc_config(&socket_path, &port_file_path, false),
    socket_path.clone(),
    port_file_path.clone(),
  );

  let result = server
    .wait_for_exit(EXIT_TIMEOUT)
    .expect("a symlink collision must make run_ipc_server fail promptly");
  let error = result.expect_err("symlink at the socket path must not be replaced");
  assert_addr_in_use(error);

  let metadata = fs::symlink_metadata(&socket_path).expect("symlink must still exist");
  assert!(
    metadata.file_type().is_symlink(),
    "path must remain a symlink, not become a socket or file"
  );
  assert_eq!(
    fs::read(&socket_path).expect("symlink must still resolve to its target"),
    contents.as_slice(),
    "symlink target contents must be untouched"
  );
  assert!(
    !port_file_path.exists(),
    "a symlink collision must be terminal, not a TCP fallback"
  );
}

#[test]
fn shutdown_command_stops_server_and_cleans_up_socket() {
  let dir = TestDir::new("shutdown");
  let socket_path = dir.socket_path();
  let port_file_path = dir.port_file_path();
  let mut server = ServerGuard::spawn(
    test_engine(),
    ipc_config(&socket_path, &port_file_path, false),
    socket_path.clone(),
    port_file_path.clone(),
  );
  wait_for_socket(&socket_path);

  let response = send_unix_command(&socket_path, &json!({"command": "shutdown"}));
  assert_eq!(
    response["status"], "ok",
    "shutdown command must be acknowledged"
  );

  let result = server
    .wait_for_exit(EXIT_TIMEOUT)
    .expect("server must exit after a shutdown command");
  result.expect("run_ipc_server must return Ok after graceful shutdown");

  assert!(
    !socket_path.exists(),
    "shutdown must remove the custom socket file"
  );
  assert!(
    !port_file_path.exists(),
    "shutdown must leave no TCP port file behind"
  );
}
