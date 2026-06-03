//! around: A cross-platform audio player — CLI entry point.

// Clippy: expect() on startup-critical operations is intentional — failure to initialise is unrecoverable.
#![allow(clippy::expect_used)]

use around_core::Source;
use around_engine::ipc::types::{IpcCommand, IpcResponse, ResponseStatus};
use around_engine::{Engine, EngineConfig, PlaybackState};
use around_source_file::FileSource;
use clap::{Parser, Subcommand};
use std::io::{BufReader, BufWriter, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Parser)]
#[command(name = "around", version, about = "A cross-platform audio player", long_about = None)]
struct Cli {
  #[command(subcommand)]
  command: Commands,

  /// Log level (error, warn, info, debug, trace)
  #[arg(long, global = true, default_value = "info")]
  log_level: String,

  /// Connect to a remote engine at <host>:<port> (FR-019)
  #[arg(long, global = true)]
  remote: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
  /// Play an audio file
  Play {
    /// Path to the audio file
    path: PathBuf,

    /// Enable TCP cross-host listener (FR-014)
    #[arg(long)]
    ipc_listen_tcp: Option<String>,

    /// Enable UDP cross-host listener (FR-014)
    #[arg(long)]
    ipc_listen_udp: Option<String>,
  },

  /// Pause playback
  Pause,

  /// Resume playback
  Resume,

  /// Seek to a position (in seconds)
  Seek {
    /// Position in seconds
    #[arg(short, long)]
    position: f64,
  },

  /// Stop playback
  Stop,

  /// Show playback status
  Status,

  /// Load a codec extension from a shared library file
  LoadCodec {
    /// Path to the shared library (.so/.dylib/.dll)
    path: PathBuf,
  },
  /// List all loaded codec extensions
  ListCodecs,
  /// Remove stale temporary files from aborted codec loads
  Cleanup,

  /// Start the engine as a daemon (blocks until SIGINT/SIGTERM)
  Serve {
    /// Run in background (daemonize)
    #[arg(long)]
    daemon: bool,
  },

  /// Send shutdown signal to a running engine
  Shutdown,
}

fn main() {
  let cli = Cli::parse();

  // Initialize tracing subscriber
  let filter = tracing_subscriber::EnvFilter::try_from_default_env()
    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&cli.log_level));
  tracing_subscriber::fmt()
    .with_env_filter(filter)
    .with_target(false)
    .init();

  let remote = cli.remote.as_deref();

  match cli.command {
    Commands::Play {
      path,
      ipc_listen_tcp,
      ipc_listen_udp,
    } => {
      if let Err(e) = cmd_play(path, ipc_listen_tcp, ipc_listen_udp, remote) {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
    Commands::Pause => {
      if let Err(e) = cmd_pause(remote) {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
    Commands::Resume => {
      if let Err(e) = cmd_resume(remote) {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
    Commands::Seek { position } => {
      if let Err(e) = cmd_seek(position, remote) {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
    Commands::Stop => {
      if let Err(e) = cmd_stop(remote) {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
    Commands::Status => {
      if let Err(e) = cmd_status(remote) {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
    Commands::LoadCodec { path } => {
      if let Err(e) = cmd_load_codec(path, remote) {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
    Commands::ListCodecs => {
      if let Err(e) = cmd_list_codecs(remote) {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
    Commands::Cleanup => {
      if let Err(e) = cmd_cleanup(remote) {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
    Commands::Serve { daemon } => {
      if let Err(e) = cmd_serve(daemon) {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
    Commands::Shutdown => {
      if let Err(e) = cmd_shutdown(remote) {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
  }
}

fn cmd_play(
  path: PathBuf,
  ipc_listen_tcp: Option<String>,
  ipc_listen_udp: Option<String>,
  remote: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
  // Auto-detect: if an engine is already running, delegate to it.
  if remote.is_none() {
    let probe = IpcCommand::Status;
    if send_ipc_command(&probe, None).is_ok() {
      let play_cmd = IpcCommand::Play {
        path: path.display().to_string(),
      };
      let resp = send_ipc_command(&play_cmd, None)?;
      if resp.status == ResponseStatus::Ok {
        println!("delegated to running engine");
        return Ok(());
      }
      eprintln!(
        "engine rejected play: {}",
        resp.message.as_deref().unwrap_or("unknown error")
      );
      std::process::exit(1);
    }
  }

  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));

  // Build IpcConfig from CLI flags.
  let ipc_config = around_engine::ipc::IpcConfig::from_cli_env(ipc_listen_tcp, ipc_listen_udp)?;

  // Shared state: the decode thread updates position/state in real-time,
  // the IPC server reads it to serve status/pause/resume/seek commands.
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  let source = FileSource::new(&path);
  let source_box: Box<dyn Source> = Box::new(source);

  // Set up Ctrl+C handler before starting playback.
  let eng = engine.clone();
  ctrlc::set_handler(move || {
    tracing::info!("received interrupt signal, stopping...");
    eng.stop();
  })?;

  tracing::debug!("signal handlers: SIGINT (ctrlc), SIGHUP/SIGTERM (tokio)");

  // Start IPC server on a background thread for control commands.
  let ipc_engine = engine.clone();
  let ipc_state = state.clone();
  let port_file = std::env::temp_dir().join("around.port");
  let ipc_handle = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .build()
      .expect("failed to create tokio runtime for IPC");
    rt.block_on(async {
      // Watch for SIGTERM and SIGHUP inside the tokio runtime.
      #[cfg(unix)]
      {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM watcher");
        let mut sighup = signal(SignalKind::hangup()).expect("SIGHUP watcher");

        let eng = ipc_engine.clone();
        tokio::spawn(async move {
          tokio::select! {
            _ = sigterm.recv() => {
              tracing::info!("received SIGTERM, stopping playback");
              eng.stop();
            }
            _ = sighup.recv() => {
              tracing::info!("received SIGHUP, stopping playback");
              eng.stop();
            }
          }
        });
      }

      if let Err(e) = around_engine::ipc::run_ipc_server(ipc_config, ipc_engine, ipc_state).await {
        tracing::error!(?e, "IPC server error");
      }
    });
  });

  tracing::info!("playing '{}'", path.display());

  // Run play() on a background thread.
  let eng = engine.clone();
  let play_state = state.clone();
  let play_thread = thread::spawn(move || eng.play(source_box, play_state));

  match play_thread.join() {
    Ok(Ok(handle)) => {
      tracing::info!(
        "playback complete ({} Hz, {} channels)",
        handle.output_format.sample_rate,
        handle.output_format.channels
      );
      drop(handle);
    }
    Ok(Err(e)) => {
      engine.shutdown();
      let _ = std::fs::read_to_string(&port_file)
        .ok()
        .and_then(|p| p.trim().parse::<u16>().ok())
        .and_then(|p| TcpStream::connect(format!("127.0.0.1:{}", p)).ok());
      let _ = ipc_handle.join();
      let _ = std::fs::remove_file(&port_file);
      return Err(Box::new(e));
    }
    Err(_panic) => {
      engine.shutdown();
      let _ = std::fs::read_to_string(&port_file)
        .ok()
        .and_then(|p| p.trim().parse::<u16>().ok())
        .and_then(|p| TcpStream::connect(format!("127.0.0.1:{}", p)).ok());
      let _ = ipc_handle.join();
      let _ = std::fs::remove_file(&port_file);
      return Err("playback thread panicked".into());
    }
  }

  // Signal IPC server to shut down, then unblock accept().
  engine.shutdown();
  let _ = std::fs::read_to_string(&port_file)
    .ok()
    .and_then(|p| p.trim().parse::<u16>().ok())
    .and_then(|p| TcpStream::connect(format!("127.0.0.1:{}", p)).ok());
  let _ = ipc_handle.join();

  let _ = std::fs::remove_file(&port_file);
  tracing::info!("playback complete, exiting");
  Ok(())
}

// ---------------------------------------------------------------------------
// IPC command helpers — auto-detect transport
// ---------------------------------------------------------------------------

/// Send a JSON command to the engine using the best available transport.
///
/// Auto-detection order (FR-006, FR-011):
///   1. If `--remote` was specified, connect directly via TCP.
///   2. Try native transport (Unix socket / named pipe).
///   3. Fall back: read port from `<temp>/around.port` and connect via TCP.
fn send_ipc_command(
  req: &IpcCommand,
  remote: Option<&str>,
) -> Result<IpcResponse, Box<dyn std::error::Error>> {
  // 1. Direct remote connection (--remote flag).
  if let Some(addr) = remote {
    let socket_addr: std::net::SocketAddr = addr.parse()?;
    let stream = TcpStream::connect_timeout(&socket_addr, std::time::Duration::from_secs(3))?;
    return send_over_tcp_stream(req, stream);
  }

  // 2. Try native transport first.
  #[cfg(unix)]
  {
    if let Ok(resp) = try_unix_socket_command(req) {
      return Ok(resp);
    }
  }
  #[cfg(windows)]
  {
    if let Ok(resp) = try_named_pipe_command(req) {
      return Ok(resp);
    }
  }

  // 3. TCP fallback via port file.
  let port_file = std::env::temp_dir().join("around.port");
  let port = std::fs::read_to_string(&port_file)
    .map_err(|_| "no engine instance running (tried native transport and port file)")?;
  let port: u16 = port
    .trim()
    .parse()
    .map_err(|_| format!("invalid port in port file: {}", port))?;

  let fallback_addr: std::net::SocketAddr = format!("127.0.0.1:{}", port).parse()?;
  let stream = TcpStream::connect_timeout(&fallback_addr, std::time::Duration::from_secs(3))?;
  send_over_tcp_stream(req, stream)
}

/// Send a JSON request over an already-established TCP stream.
fn send_over_tcp_stream(
  req: &IpcCommand,
  stream: TcpStream,
) -> Result<IpcResponse, Box<dyn std::error::Error>> {
  let mut reader = BufReader::new(&stream);
  let mut writer = BufWriter::new(&stream);

  let request = serde_json::to_string(req)?;
  writer.write_all(request.as_bytes())?;
  writer.write_all(b"\n")?;
  writer.flush()?;
  stream.shutdown(std::net::Shutdown::Write)?;

  let mut buf = String::new();
  reader.read_to_string(&mut buf)?;

  let response: IpcResponse = serde_json::from_str(&buf)?;
  Ok(response)
}

/// Try to send a command via the Unix domain socket (Linux/macOS).
#[cfg(unix)]
fn try_unix_socket_command(req: &IpcCommand) -> Result<IpcResponse, Box<dyn std::error::Error>> {
  use std::os::unix::net::UnixStream;

  let socket_path = resolve_unix_socket_path();
  let stream = UnixStream::connect(&socket_path)?;

  let mut reader = BufReader::new(&stream);
  let mut writer = BufWriter::new(&stream);

  let request = serde_json::to_string(req)?;
  writer.write_all(request.as_bytes())?;
  writer.write_all(b"\n")?;
  writer.flush()?;
  stream.shutdown(std::net::Shutdown::Write)?;

  let mut buf = String::new();
  reader.read_to_string(&mut buf)?;

  let response: IpcResponse = serde_json::from_str(&buf)?;
  Ok(response)
}

/// Resolve the Unix socket path (mirrors engine's resolve_socket_dir).
#[cfg(unix)]
fn resolve_unix_socket_path() -> std::path::PathBuf {
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

/// Try to send a command via the Windows named pipe.
#[cfg(windows)]
fn try_named_pipe_command(req: &IpcCommand) -> Result<IpcResponse, Box<dyn std::error::Error>> {
  // Open the named pipe as a file.
  let pipe = std::fs::OpenOptions::new()
    .read(true)
    .write(true)
    .open(r"\\.\pipe\around")?;

  let mut reader = BufReader::new(&pipe);
  let mut writer = BufWriter::new(&pipe);

  let request = serde_json::to_string(req)?;
  writer.write_all(request.as_bytes())?;
  writer.write_all(b"\n")?;
  writer.flush()?;
  drop(writer);

  let mut buf = String::new();
  reader.read_to_string(&mut buf)?;

  let response: IpcResponse = serde_json::from_str(&buf)?;
  Ok(response)
}

// ---------------------------------------------------------------------------
// Command implementations
// ---------------------------------------------------------------------------

fn cmd_pause(remote: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
  let req = IpcCommand::Pause;
  let resp = send_ipc_command(&req, remote)?;
  match resp.status {
    ResponseStatus::Ok => println!("Paused."),
    ResponseStatus::Error => eprintln!("Error: {}", resp.message.as_deref().unwrap_or("unknown")),
  }
  Ok(())
}

fn cmd_resume(remote: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
  let req = IpcCommand::Resume;
  let resp = send_ipc_command(&req, remote)?;
  match resp.status {
    ResponseStatus::Ok => println!("Resumed."),
    ResponseStatus::Error => eprintln!("Error: {}", resp.message.as_deref().unwrap_or("unknown")),
  }
  Ok(())
}

fn cmd_seek(position: f64, remote: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
  if position < 0.0 {
    return Err("position must be non-negative".into());
  }
  let position_ms = (position * 1000.0) as u64;
  let req = IpcCommand::Seek { position_ms };
  let resp = send_ipc_command(&req, remote)?;
  match resp.status {
    ResponseStatus::Ok => println!("Seeked to {}s.", position),
    ResponseStatus::Error => eprintln!("Error: {}", resp.message.as_deref().unwrap_or("unknown")),
  }
  Ok(())
}

fn cmd_stop(remote: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
  let req = IpcCommand::Stop;
  let resp = send_ipc_command(&req, remote)?;
  match resp.status {
    ResponseStatus::Ok => println!("Stopped."),
    ResponseStatus::Error => eprintln!("Error: {}", resp.message.as_deref().unwrap_or("unknown")),
  }
  Ok(())
}

fn cmd_status(remote: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
  let req = IpcCommand::Status;
  let resp = send_ipc_command(&req, remote)?;
  if resp.status == ResponseStatus::Ok {
    if let Some(track) = &resp.track {
      println!(
        "State: {}",
        resp
          .state
          .map(|s| s.to_string())
          .unwrap_or_else(|| "unknown".into())
      );
      println!("Track: {}", track.path);
      println!("Format: {} ({} ms)", track.format, track.duration_ms);
      if let Some(pos) = resp.position_ms {
        println!("Position: {}ms", pos);
      }
    } else {
      println!("State: idle");
    }
  } else {
    eprintln!("Error: {}", resp.message.as_deref().unwrap_or("unknown"));
  }
  Ok(())
}

fn cmd_load_codec(path: PathBuf, remote: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
  let req = IpcCommand::LoadCodec {
    path: path.display().to_string(),
  };
  let resp = send_ipc_command(&req, remote)?;
  match resp.status {
    ResponseStatus::Ok => println!("Codec loaded: {}", path.display()),
    ResponseStatus::Error => {
      let msg = resp.message.as_deref().unwrap_or("unknown error");
      return Err(msg.into());
    }
  }
  Ok(())
}

fn cmd_list_codecs(remote: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
  let req = IpcCommand::ListCodecs;
  let resp = send_ipc_command(&req, remote)?;
  if resp.status == ResponseStatus::Ok {
    if let Some(codecs) = &resp.codecs {
      for d in codecs {
        let formats = d.formats.join(", ");
        println!("{} [{}] ({})", d.name, formats, d.source);
      }
    } else {
      println!("no codecs loaded");
    }
    Ok(())
  } else {
    let msg = resp.message.as_deref().unwrap_or("unknown error");
    Err(msg.into())
  }
}

/// fork + setsid to detach from controlling terminal.
///
/// SAFETY: Must be called before any tokio runtime is created.
/// The parent process exits immediately; the child continues.
#[cfg(unix)]
fn daemonize() -> Result<(), Box<dyn std::error::Error>> {
  match unsafe { libc::fork() } {
    -1 => Err("fork failed".into()),
    0 => {
      // Child: create new session, detach from controlling terminal.
      if unsafe { libc::setsid() } == -1 {
        return Err("setsid failed".into());
      }
      Ok(())
    }
    _ => std::process::exit(0),
  }
}

/// Re-spawn as a detached process with no console.
/// Fork + setsid to detach from controlling terminal.
///
/// The parent exits immediately; the child (re-spawned) continues
/// with environment variable AROUND_DAEMON_CHILD=1 set.
#[cfg(windows)]
fn daemonize() -> Result<(), Box<dyn std::error::Error>> {
  use std::os::windows::process::CommandExt;
  let exe = std::env::current_exe()?;
  let mut cmd = std::process::Command::new(exe);
  cmd
    .args(std::env::args().skip(1))
    .env("AROUND_DAEMON_CHILD", "1")
    .creation_flags(0x00000008) // DETACHED_PROCESS
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null());
  cmd.spawn()?;
  std::process::exit(0);
}

fn cmd_serve(daemon: bool) -> Result<(), Box<dyn std::error::Error>> {
  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  let ipc_config = around_engine::ipc::IpcConfig::default();

  if daemon {
    if std::env::var("AROUND_DAEMON_CHILD").is_ok() {
      // Already the re-spawned child process (Windows path).
      // Fall through to normal serve below.
    } else {
      daemonize()?;
      // If we reach here, we're the child process (Unix fork)
      // or a re-spawn would have exited the parent (Windows).
    }
  }

  // Normal serve (foreground or daemon child).
  let shutdown_engine = engine.clone();
  ctrlc::set_handler(move || {
    shutdown_engine.shutdown();
  })?;

  let rt = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()?;
  rt.block_on(around_engine::ipc::run_ipc_server(
    ipc_config, engine, state,
  ))
  .map_err(|e| -> Box<dyn std::error::Error> { e })?;
  Ok(())
}

fn cmd_shutdown(remote: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
  let req = IpcCommand::Shutdown;
  let _resp = send_ipc_command(&req, remote)?;
  println!("Shutdown sent.");
  Ok(())
}

fn cmd_cleanup(remote: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
  let req = IpcCommand::Cleanup;
  let resp = send_ipc_command(&req, remote)?;
  match resp.status {
    ResponseStatus::Ok => {
      if let Some(files) = &resp.removed_files {
        for f in files {
          println!("removed: {}", f);
        }
      } else {
        println!("Cleanup complete.");
      }
      Ok(())
    }
    ResponseStatus::Error => {
      let msg = resp.message.as_deref().unwrap_or("unknown error");
      Err(msg.into())
    }
  }
}
