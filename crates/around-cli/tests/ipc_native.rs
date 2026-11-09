//! Real CLI discovery and exit codes against a native IPC engine.
#![cfg(unix)]
#![expect(
  clippy::expect_used,
  reason = "isolated temporary paths, child processes and JSON fixtures are test invariants"
)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);
const DEADLINE: Duration = Duration::from_secs(5);

struct Sandbox(PathBuf);

impl Sandbox {
  fn new() -> Self {
    let path = std::env::temp_dir().join(format!(
      "ac-{:x}-{:x}",
      std::process::id(),
      NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&path).expect("unique sandbox must be creatable");
    Self(path)
  }
}

impl Drop for Sandbox {
  fn drop(&mut self) {
    let _ = std::fs::remove_dir_all(&self.0);
  }
}

struct Process(Child);

impl Drop for Process {
  fn drop(&mut self) {
    // kill() is harmless after exit; wait() always reaps the process on panic.
    let _ = self.0.kill();
    let _ = self.0.wait();
  }
}

fn command(runtime: &Path, temporary: &Path) -> Command {
  let mut command = Command::new(env!("CARGO_BIN_EXE_around"));
  command
    .env("XDG_RUNTIME_DIR", runtime)
    .env("TMPDIR", temporary)
    .env_remove("AROUND_IPC_TCP_BIND")
    .env_remove("AROUND_IPC_UDP_BIND")
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
  command
}

fn run(mut command: Command) -> Output {
  let mut child = Process(command.spawn().expect("CLI must launch"));
  let end = Instant::now() + DEADLINE;
  loop {
    if child
      .0
      .try_wait()
      .expect("child status must be readable")
      .is_some()
    {
      // Take the pipes without moving Child out of its cleanup guard.
      let mut stdout = Vec::new();
      let mut stderr = Vec::new();
      use std::io::Read;
      child
        .0
        .stdout
        .take()
        .expect("stdout pipe")
        .read_to_end(&mut stdout)
        .expect("read stdout");
      child
        .0
        .stderr
        .take()
        .expect("stderr pipe")
        .read_to_end(&mut stderr)
        .expect("read stderr");
      return Output {
        status: child.0.wait().expect("child must be reaped"),
        stdout,
        stderr,
      };
    }
    assert!(
      Instant::now() < end,
      "CLI failed to exit within {DEADLINE:?}"
    );
    std::thread::sleep(Duration::from_millis(10));
  }
}

fn wait_for_server(server: &mut Process, endpoint: &Path) {
  let end = Instant::now() + DEADLINE;
  loop {
    assert!(
      server.0.try_wait().expect("server status").is_none(),
      "server exited before binding"
    );
    if let Ok(mut stream) = UnixStream::connect(endpoint) {
      stream
        .set_read_timeout(Some(DEADLINE))
        .expect("read timeout");
      stream
        .set_write_timeout(Some(DEADLINE))
        .expect("write timeout");
      stream
        .write_all(b"{\"command\":\"status\"}\n")
        .expect("status request");
      let mut line = String::new();
      BufReader::new(stream)
        .read_line(&mut line)
        .expect("status response");
      let response: serde_json::Value = serde_json::from_str(&line).expect("JSON response");
      assert_eq!(response["status"], "ok");
      return;
    }
    assert!(
      Instant::now() < end,
      "server failed to bind fallback endpoint"
    );
    std::thread::sleep(Duration::from_millis(10));
  }
}

#[test]
fn cli_discovers_native_fallback_and_propagates_engine_errors() {
  // Cover directory preparation failure and bind failure separately. Neither
  // case may leave a listening engine unreachable by the CLI.
  for bind_failure in [false, true] {
    let sandbox = Sandbox::new();
    let temporary = sandbox.0.join("tmp");
    std::fs::create_dir(&temporary).expect("temporary directory");
    let runtime = sandbox.0.join(if bind_failure {
      "runtime-path-longer-than-a-Unix-socket-can-bind-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
    } else {
      "runtime-file"
    });
    if bind_failure {
      std::fs::create_dir(&runtime).expect("long runtime directory");
    } else {
      std::fs::write(&runtime, b"not a directory").expect("blocked runtime directory");
    }
    let endpoint = temporary.join("around.sock");
    let mut serve = command(&runtime, &temporary);
    serve.arg("serve");
    let mut server = Process(serve.spawn().expect("engine must launch"));
    wait_for_server(&mut server, &endpoint);

    let mut status = command(&runtime, &temporary);
    status.arg("status");
    let output = run(status);
    assert!(
      output.status.success(),
      "{}",
      String::from_utf8_lossy(&output.stderr)
    );
    assert!(
      !temporary.join("around.port").exists(),
      "native fallback must not require TCP"
    );

    for args in [
      vec!["pause"],
      vec!["resume"],
      vec!["seek", "--position", "0"],
      vec!["status", "--stream-id", "1"],
    ] {
      let mut client = command(&runtime, &temporary);
      client.args(&args);
      let output = run(client);
      assert_eq!(output.status.code(), Some(1), "{args:?}");
      assert!(
        output.stdout.is_empty(),
        "failed command must not report success: {args:?}"
      );
    }

    let mut duplicate = command(&runtime, &temporary);
    duplicate.arg("serve");
    let output = run(duplicate);
    assert_eq!(output.status.code(), Some(1));

    let mut stop = command(&runtime, &temporary);
    stop.arg("stop");
    assert!(run(stop).status.success());
    let mut status = command(&runtime, &temporary);
    status.arg("status");
    assert!(
      run(status).status.success(),
      "Stop must leave IPC reachable"
    );

    let mut shutdown = command(&runtime, &temporary);
    shutdown.arg("shutdown");
    assert!(run(shutdown).status.success());
    let end = Instant::now() + DEADLINE;
    while server.0.try_wait().expect("server status").is_none() {
      assert!(Instant::now() < end, "server failed to shut down");
      std::thread::sleep(Duration::from_millis(10));
    }
    assert!(server.0.wait().expect("server exit").success());
    assert!(
      !endpoint.exists(),
      "actual fallback endpoint must be removed"
    );
    if !bind_failure {
      assert_eq!(
        std::fs::read(&runtime).expect("runtime file must survive"),
        b"not a directory"
      );
    }
  }
}

#[test]
fn concurrent_stale_socket_takeover_has_one_owner() {
  for _ in 0..12 {
    let sandbox = Sandbox::new();
    let runtime = sandbox.0.join("run");
    let temporary = sandbox.0.join("tmp");
    std::fs::create_dir_all(runtime.join("around")).expect("runtime directory");
    std::fs::create_dir(&temporary).expect("temporary directory");
    let endpoint = runtime.join("around/around.sock");
    // All contenders observe the same endpoint left by a crashed process.
    drop(std::os::unix::net::UnixListener::bind(&endpoint).expect("stale socket"));

    let mut contenders: Vec<Process> = (0..3)
      .map(|_| {
        let mut serve = command(&runtime, &temporary);
        serve.arg("serve");
        Process(serve.spawn().expect("contender must launch"))
      })
      .collect();
    let end = Instant::now() + DEADLINE;
    let winner = loop {
      let mut live = Vec::new();
      for (index, process) in contenders.iter_mut().enumerate() {
        match process.0.try_wait().expect("contender status") {
          None => live.push(index),
          Some(status) => assert_eq!(status.code(), Some(1), "loser must report failure"),
        }
      }
      assert!(!live.is_empty(), "one contender must own the endpoint");
      if live.len() == 1 {
        break live[0];
      }
      assert!(
        Instant::now() < end,
        "multiple engines claimed the same socket"
      );
      std::thread::sleep(Duration::from_millis(10));
    };
    wait_for_server(&mut contenders[winner], &endpoint);
    let mut shutdown = command(&runtime, &temporary);
    shutdown.arg("shutdown");
    assert!(run(shutdown).status.success());
    let end = Instant::now() + DEADLINE;
    while contenders[winner]
      .0
      .try_wait()
      .expect("winner status")
      .is_none()
    {
      assert!(Instant::now() < end, "winner failed to shut down");
      std::thread::sleep(Duration::from_millis(10));
    }
    assert!(contenders[winner].0.wait().expect("winner exit").success());
    assert!(!endpoint.exists(), "winner must clean its endpoint");
    assert!(
      !temporary.join("around.port").exists(),
      "losers must not fall back to TCP"
    );
  }
}
