//! around: A cross-platform audio player — CLI entry point.

// Clippy: expect() on startup-critical operations is intentional — failure to
// initialise is unrecoverable. Each function using expect() carries its own
// #[expect] annotation documenting the specific invariant.

use around_engine::ipc::types::{IpcCommand, IpcResponse, ResponseStatus};
use around_engine::{Engine, EngineConfig};
use clap::{Parser, Subcommand};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;
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

    /// Stream ID to use (optional)
    #[arg(long)]
    stream_id: Option<u64>,
  },

  /// Pause playback
  Pause {
    /// Stream ID to pause (optional, pauses sole active stream)
    #[arg(long)]
    stream_id: Option<u64>,
  },

  /// Resume playback
  Resume {
    /// Stream ID to resume (optional, resumes sole active stream)
    #[arg(long)]
    stream_id: Option<u64>,
  },

  /// Seek to a position (in seconds)
  Seek {
    /// Position in seconds
    #[arg(short, long)]
    position: f64,

    /// Stream ID to seek (optional, seeks sole active stream)
    #[arg(long)]
    stream_id: Option<u64>,
  },

  /// Stop playback
  Stop {
    /// Stream ID to stop (optional, stops all playback)
    #[arg(long)]
    stream_id: Option<u64>,
  },

  /// Show playback status
  Status {
    /// Stream ID (optional, shows all streams)
    #[arg(long)]
    stream_id: Option<u64>,
  },

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
      stream_id,
    } => {
      if let Err(e) = cmd_play(path, ipc_listen_tcp, ipc_listen_udp, stream_id, remote) {
        eprintln!("error: {e}");
        std::process::exit(1);
      }
    }
    Commands::Pause { stream_id } => {
      if let Err(e) = cmd_pause(stream_id, remote) {
        eprintln!("error: {e}");
        std::process::exit(1);
      }
    }
    Commands::Resume { stream_id } => {
      if let Err(e) = cmd_resume(stream_id, remote) {
        eprintln!("error: {e}");
        std::process::exit(1);
      }
    }
    Commands::Seek {
      position,
      stream_id,
    } => {
      if let Err(e) = cmd_seek(position, stream_id, remote) {
        eprintln!("error: {e}");
        std::process::exit(1);
      }
    }
    Commands::Stop { stream_id } => {
      if let Err(e) = cmd_stop(stream_id, remote) {
        eprintln!("error: {e}");
        std::process::exit(1);
      }
    }
    Commands::Status { stream_id } => {
      if let Err(e) = cmd_status(stream_id, remote) {
        eprintln!("error: {e}");
        std::process::exit(1);
      }
    }
    Commands::LoadCodec { path } => {
      if let Err(e) = cmd_load_codec(path, remote) {
        eprintln!("error: {e}");
        std::process::exit(1);
      }
    }
    Commands::ListCodecs => {
      if let Err(e) = cmd_list_codecs(remote) {
        eprintln!("error: {e}");
        std::process::exit(1);
      }
    }
    Commands::Cleanup => {
      if let Err(e) = cmd_cleanup(remote) {
        eprintln!("error: {e}");
        std::process::exit(1);
      }
    }
    Commands::Serve { daemon } => {
      if let Err(e) = cmd_serve(daemon) {
        eprintln!("error: {e}");
        std::process::exit(1);
      }
    }
    Commands::Shutdown => {
      if let Err(e) = cmd_shutdown(remote) {
        eprintln!("error: {e}");
        std::process::exit(1);
      }
    }
  }
}

// Single expect() on tokio runtime creation and signal handlers is intentional:
// failure to initialize runtime or signal watchers is unrecoverable at startup.
fn cmd_play(
  path: PathBuf,
  ipc_listen_tcp: Option<String>,
  ipc_listen_udp: Option<String>,
  stream_id: Option<u64>,
  remote: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
  // Auto-detect: if an engine is already running, delegate to it.
  if remote.is_none() {
    let probe = IpcCommand::Status { stream_id: None };
    if send_ipc_command(&probe, None).is_ok() {
      let play_cmd = IpcCommand::Play {
        path: path.display().to_string(),
        stream_id,
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

  // Set up Ctrl+C handler before starting playback.
  let eng = engine.clone();
  ctrlc::set_handler(move || {
    tracing::info!("received interrupt signal, stopping...");
    eng.stop();
  })?;

  tracing::debug!("signal handlers: SIGINT (ctrlc), SIGHUP/SIGTERM (tokio)");

  // Start IPC server on a background thread for control commands.
  let ipc_engine = engine.clone();
  let port_file = std::env::temp_dir().join("around.port");
  let ipc_handle = std::thread::spawn(move || {
    #[expect(
      clippy::expect_used,
      reason = "tokio runtime creation always succeeds with basic config — panic is correct on env failure"
    )]
    let rt = tokio::runtime::Builder::new_current_thread()
      .enable_io()
      .build()
      .expect("failed to create tokio runtime for IPC");
    rt.block_on(async {
      // Watch for SIGTERM and SIGHUP inside the tokio runtime.
      #[cfg(unix)]
      {
        use tokio::signal::unix::{signal, SignalKind};
        #[expect(
          clippy::expect_used,
          reason = "SIGTERM watcher creation always succeeds on unix — panic is correct on env failure"
        )]
        let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM watcher");
        #[expect(
          clippy::expect_used,
          reason = "SIGHUP watcher creation always succeeds on unix — panic is correct on env failure"
        )]
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

      if let Err(e) = around_engine::ipc::run_ipc_server(ipc_config, ipc_engine).await {
        tracing::error!(?e, "IPC server error");
      }
    });
  });

  tracing::info!("playing '{}'", path.display());

  // Use prepare() + run_stream() instead of the deprecated play().
  let source = around_source_file::FileSource::new(&path);
  let prepared = match engine.prepare(Box::new(source)) {
    Ok(p) => p,
    Err(e) => {
      engine.shutdown();
      let _ = std::fs::read_to_string(&port_file)
        .ok()
        .and_then(|p| p.trim().parse::<u16>().ok())
        .and_then(|p| TcpStream::connect(format!("127.0.0.1:{p}")).ok());
      let _ = ipc_handle.join();
      let _ = std::fs::remove_file(&port_file);
      return Err(Box::new(e));
    }
  };

  let sid = prepared.stream_id;

  // Run the blocking decode loop on a background thread.
  let eng = engine.clone();
  let play_thread = thread::spawn(move || eng.run_stream(sid));

  match play_thread.join() {
    Ok(result) => {
      if let Err(e) = result {
        engine.shutdown();
        let _ = std::fs::read_to_string(&port_file)
          .ok()
          .and_then(|p| p.trim().parse::<u16>().ok())
          .and_then(|p| TcpStream::connect(format!("127.0.0.1:{p}")).ok());
        let _ = ipc_handle.join();
        let _ = std::fs::remove_file(&port_file);
        return Err(Box::new(e));
      }
      tracing::info!(
        "playback complete ({} Hz, {} channels)",
        prepared.output_format.sample_rate,
        prepared.output_format.channels
      );
    }
    Err(_panic) => {
      engine.shutdown();
      let _ = std::fs::read_to_string(&port_file)
        .ok()
        .and_then(|p| p.trim().parse::<u16>().ok())
        .and_then(|p| TcpStream::connect(format!("127.0.0.1:{p}")).ok());
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
    .and_then(|p| TcpStream::connect(format!("127.0.0.1:{p}")).ok());
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
  if let Some(addr) = remote {
    let stream = TcpStream::connect(addr)?;
    return send_over_tcp_stream(req, stream);
  }

  // Try native transport first.
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

  // Fallback: read port file and connect via TCP.
  let port_path = std::env::temp_dir().join("around.port");
  let port_str = std::fs::read_to_string(&port_path)?;
  let port: u16 = port_str.trim().parse()?;
  let stream = TcpStream::connect(format!("127.0.0.1:{port}"))?;
  send_over_tcp_stream(req, stream)
}

/// Send a JSON request over an already-established TCP stream.
fn send_over_tcp_stream(
  req: &IpcCommand,
  stream: TcpStream,
) -> Result<IpcResponse, Box<dyn std::error::Error>> {
  let mut reader = BufReader::new(&stream);
  let mut writer = BufWriter::new(&stream);

  let json = serde_json::to_vec(req)?;
  writer.write_all(&json)?;
  writer.write_all(b"\n")?;
  writer.flush()?;

  let mut response = String::new();
  reader.read_line(&mut response)?;
  Ok(serde_json::from_str(response.trim_end())?)
}

/// Try to send a command via the Unix domain socket (Linux/macOS).
#[cfg(unix)]
fn try_unix_socket_command(req: &IpcCommand) -> Result<IpcResponse, Box<dyn std::error::Error>> {
  use std::os::unix::net::UnixStream;

  let socket_path = resolve_unix_socket_path();
  let stream = UnixStream::connect(&socket_path)?;
  let mut reader = BufReader::new(&stream);
  let mut writer = BufWriter::new(&stream);

  let json = serde_json::to_vec(req)?;
  writer.write_all(&json)?;
  writer.write_all(b"\n")?;
  writer.flush()?;

  let mut response = String::new();
  reader.read_line(&mut response)?;
  Ok(serde_json::from_str(response.trim_end())?)
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
  // Windows named pipe implementation for IPC.
  use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
  use windows::Win32::Foundation::HANDLE;
  use windows::Win32::Storage::FileSystem::CreateFileA;
  use windows::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;
  use windows::Win32::Storage::FileSystem::GENERIC_READ;
  use windows::Win32::Storage::FileSystem::GENERIC_WRITE;
  use windows::Win32::Storage::FileSystem::OPEN_EXISTING;

  let pipe_name = r"\\.\pipe\around\0";
  let handle = unsafe {
    CreateFileA(
      pipe_name,
      GENERIC_READ | GENERIC_WRITE,
      windows::Win32::Storage::FileSystem::FILE_SHARE_NONE,
      None,
      OPEN_EXISTING,
      FILE_FLAG_OVERLAPPED,
      HANDLE::default(),
    )
  };

  if handle.is_invalid() {
    return Err("Failed to connect to named pipe".into());
  }

  // Use tokio's NamedPipeClient for async I/O.
  let rt = tokio::runtime::Builder::new_current_thread()
    .enable_io()
    .build()?;
  rt.block_on(async {
    let client = tokio::net::windows::named_pipe::NamedPipeClient::new(pipe_name)?;
    client.connect().await?;

    let (reader, mut writer) = tokio::io::split(client);
    let mut reader = BufReader::new(reader);

    let json = serde_json::to_vec(req)?;
    writer.write_all(&json).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    let mut response = String::new();
    reader.read_line(&mut response).await?;
    Ok(serde_json::from_str(response.trim_end())?)
  })
}

// ---------------------------------------------------------------------------
// Command implementations
// ---------------------------------------------------------------------------

fn cmd_pause(
  stream_id: Option<u64>,
  remote: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
  let req = IpcCommand::Pause { stream_id };
  let resp = send_ipc_command(&req, remote)?;
  match resp.status {
    ResponseStatus::Ok => println!("Paused."),
    ResponseStatus::Error => eprintln!("Error: {}", resp.message.as_deref().unwrap_or("unknown")),
  }
  Ok(())
}

fn cmd_resume(
  stream_id: Option<u64>,
  remote: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
  let req = IpcCommand::Resume { stream_id };
  let resp = send_ipc_command(&req, remote)?;
  match resp.status {
    ResponseStatus::Ok => println!("Resumed."),
    ResponseStatus::Error => eprintln!("Error: {}", resp.message.as_deref().unwrap_or("unknown")),
  }
  Ok(())
}

fn cmd_seek(
  position: f64,
  stream_id: Option<u64>,
  remote: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
  if !position.is_finite() {
    return Err("position must be finite".into());
  }
  if position < 0.0 {
    return Err("position must be non-negative".into());
  }
  let ms_f64 = position * 1000.0;
  if ms_f64 >= u64::MAX as f64 {
    return Err("position too large".into());
  }
  let position_ms = ms_f64 as u64;
  let req = IpcCommand::Seek {
    position_ms,
    stream_id,
  };
  let resp = send_ipc_command(&req, remote)?;
  match resp.status {
    ResponseStatus::Ok => println!("Seeked to {position}s."),
    ResponseStatus::Error => eprintln!("Error: {}", resp.message.as_deref().unwrap_or("unknown")),
  }
  Ok(())
}

fn cmd_stop(
  stream_id: Option<u64>,
  remote: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
  let req = IpcCommand::Stop { stream_id };
  let resp = send_ipc_command(&req, remote)?;
  match resp.status {
    ResponseStatus::Ok => println!("Stopped."),
    ResponseStatus::Error => eprintln!("Error: {}", resp.message.as_deref().unwrap_or("unknown")),
  }
  Ok(())
}

fn cmd_status(
  stream_id: Option<u64>,
  remote: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
  let req = IpcCommand::Status { stream_id };
  let resp = send_ipc_command(&req, remote)?;
  if resp.status == ResponseStatus::Ok {
    // Multi-stream output
    if let Some(streams) = resp.streams.as_ref().filter(|streams| !streams.is_empty()) {
      for s in streams {
        println!(
          "Stream {}: {:?} at {}ms (seekable: {}, device_lost: {})",
          s.stream_id, s.status, s.position_ms, s.seekable, s.device_lost,
        );
        if let Some(ref track) = s.track {
          println!(
            "  Track: {} ({} format, {}ms)",
            track.path, track.format, track.duration_ms
          );
        }
        if let Some(content_type) = &s.content_type {
          println!("  Content-Type: {content_type}");
        }
      }
    } else if let Some(track) = &resp.track {
      // Legacy single-stream output
      println!(
        "State: {:?}",
        resp
          .state
          .unwrap_or(around_engine::ipc::types::PlaybackStatus::Stopped)
      );
      println!("Track: {}", track.path);
      println!("Format: {} ({} ms)", track.format, track.duration_ms);
      if let Some(content_type) = &resp.content_type {
        println!("Content-Type: {content_type}");
      }
      if let Some(pos) = resp.position_ms {
        println!("Position: {pos}ms");
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
  use std::os::unix::process::CommandExt;
  let args: Vec<String> = std::env::args().collect();
  let child = std::process::Command::new(&args[0])
    .args(&args[1..])
    .env("AROUND_DAEMON_CHILD", "1")
    .process_group(0)
    .spawn()?;
  println!("Daemonized with PID {}", child.id());
  std::process::exit(0);
}

/// Re-spawn as a detached process with no console.
/// Fork + setsid to detach from controlling terminal.
///
/// The parent exits immediately; the child (re-spawned) continues
/// with environment variable AROUND_DAEMON_CHILD=1 set.
#[cfg(windows)]
fn daemonize() -> Result<(), Box<dyn std::error::Error>> {
  // On Windows, daemonize by spawning a detached process.
  let args: Vec<String> = std::env::args().collect();
  let child = std::process::Command::new(&args[0])
    .args(&args[1..])
    .env("AROUND_DAEMON_CHILD", "1")
    .creation_flags(std::process::CreationFlags::CREATE_NO_WINDOW)
    .spawn()?;
  println!("Daemonized with PID {}", child.id());
  std::process::exit(0);
}

fn cmd_serve(daemon: bool) -> Result<(), Box<dyn std::error::Error>> {
  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));

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
  rt.block_on(around_engine::ipc::run_ipc_server(ipc_config, engine))
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
          println!("removed: {f}");
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
