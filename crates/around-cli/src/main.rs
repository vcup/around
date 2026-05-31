//! around: A cross-platform audio player — CLI entry point.

use around_core::Source;
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
}

#[derive(Subcommand)]
enum Commands {
  /// Play an audio file
  Play {
    /// Path to the audio file
    path: PathBuf,
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

  /// Load a decoder extension from a shared library file
  LoadDecoder {
    /// Path to the shared library (.so/.dylib/.dll)
    path: PathBuf,
  },
  /// List all loaded decoder extensions
  ListDecoders,
  /// Remove stale temporary files from aborted decoder loads
  Cleanup,
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

  match cli.command {
    Commands::Play { path } => {
      if let Err(e) = cmd_play(path) {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
    Commands::Pause => {
      if let Err(e) = cmd_pause() {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
    Commands::Resume => {
      if let Err(e) = cmd_resume() {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
    Commands::Seek { position } => {
      if let Err(e) = cmd_seek(position) {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
    Commands::Stop => {
      if let Err(e) = cmd_stop() {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
    Commands::Status => {
      if let Err(e) = cmd_status() {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
    Commands::LoadDecoder { path } => {
      if let Err(e) = cmd_load_decoder(path) {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
    Commands::ListDecoders => {
      if let Err(e) = cmd_list_decoders() {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
    Commands::Cleanup => {
      if let Err(e) = cmd_cleanup() {
        eprintln!("error: {}", e);
        std::process::exit(1);
      }
    }
  }
}

fn cmd_play(path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));

  // Shared state: the decode thread updates position/state in real-time,
  // the IPC server reads it to serve status/pause/resume/seek commands.
  let state = Arc::new(Mutex::new(PlaybackState::default()));

  let source = FileSource::new(&path);
  let source_box: Box<dyn Source> = Box::new(source);

  // Set up Ctrl+C handler before starting playback.
  // The ctrlc crate runs handlers on a dedicated signal thread,
  // so calling engine.stop() from the handler is safe.
  let eng = engine.clone();
  ctrlc::set_handler(move || {
    tracing::info!("received interrupt signal, stopping...");
    eng.stop();
  })?;
  // Ctrl+C handler (SIGINT) via ctrlc crate.
  // SIGTERM and SIGHUP are handled via tokio signal watchers inside the IPC thread.
  tracing::debug!("signal handlers: SIGINT (ctrlc), SIGHUP/SIGTERM (tokio)");

  // Start IPC server on a background thread for control commands (pause/stop/status).
  // Also watches for SIGTERM and SIGHUP to trigger graceful shutdown.
  let ipc_engine = engine.clone();
  let ipc_state = state.clone();
  // Port file for TCP IPC — the server writes its port here so clients can discover it.
  let port_file = std::env::temp_dir().join("around.port");
  let port_file_path = port_file.to_str().unwrap().to_string();
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

      if let Err(e) =
        around_engine::ipc::run_ipc_server(&port_file_path, ipc_engine, ipc_state).await
      {
        tracing::error!(?e, "IPC server error");
      }
    });
  });

  tracing::info!("playing '{}'", path.display());

  // Run play() on a background thread — it blocks until stop() or natural completion.
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
      // Signal IPC to shut down before returning the error.
      engine.signal_shutdown();
      // Connect to our own server to unblock the accept() loop.
      let _ = std::fs::read_to_string(&port_file)
        .ok()
        .and_then(|p| p.trim().parse::<u16>().ok())
        .and_then(|p| TcpStream::connect(format!("127.0.0.1:{}", p)).ok());
      let _ = ipc_handle.join();
      let _ = std::fs::remove_file(&port_file);
      return Err(Box::new(e));
    }
    Err(_panic) => {
      engine.signal_shutdown();
      let _ = std::fs::read_to_string(&port_file)
        .ok()
        .and_then(|p| p.trim().parse::<u16>().ok())
        .and_then(|p| TcpStream::connect(format!("127.0.0.1:{}", p)).ok());
      let _ = ipc_handle.join();
      let _ = std::fs::remove_file(&port_file);
      return Err("playback thread panicked".into());
    }
  }

  // Signal IPC server to shut down, then unblock accept() by connecting.
  engine.signal_shutdown();
  let _ = std::fs::read_to_string(&port_file)
    .ok()
    .and_then(|p| p.trim().parse::<u16>().ok())
    .and_then(|p| TcpStream::connect(format!("127.0.0.1:{}", p)).ok());
  let _ = ipc_handle.join();

  // Clean up port file after IPC server has exited.
  let _ = std::fs::remove_file(&port_file);
  tracing::info!("playback complete, exiting");
  Ok(())
}

/// Send a JSON command to the engine via TCP and read the response.
///
/// Reads the engine's port from `temp_dir/around.port`, connects via TCP,
/// sends the JSON request newline-terminated, shuts down the write half,
/// and reads the JSON response.
fn send_ipc_command(
  req: &serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
  let port_file = std::env::temp_dir().join("around.port");
  let port = std::fs::read_to_string(&port_file)
    .map_err(|_| "no engine instance running (port file not found)")?;
  let port: u16 = port
    .trim()
    .parse()
    .map_err(|_| format!("invalid port in port file: {}", port))?;

  let stream = TcpStream::connect(format!("127.0.0.1:{}", port))?;
  let mut reader = BufReader::new(&stream);
  let mut writer = BufWriter::new(&stream);

  let request = req.to_string();
  writer.write_all(request.as_bytes())?;
  writer.write_all(b"\n")?;
  writer.flush()?;
  stream.shutdown(std::net::Shutdown::Write)?;

  let mut buf = String::new();
  reader.read_to_string(&mut buf)?;

  let response: serde_json::Value = serde_json::from_str(&buf)?;
  Ok(response)
}

fn cmd_pause() -> Result<(), Box<dyn std::error::Error>> {
  let req = serde_json::json!({"command": "pause"});
  let resp = send_ipc_command(&req)?;
  println!("{}", resp);
  Ok(())
}

fn cmd_resume() -> Result<(), Box<dyn std::error::Error>> {
  let req = serde_json::json!({"command": "resume"});
  let resp = send_ipc_command(&req)?;
  println!("{}", resp);
  Ok(())
}

fn cmd_seek(position: f64) -> Result<(), Box<dyn std::error::Error>> {
  let position_ms = (position * 1000.0) as u64;
  let req = serde_json::json!({"command": "seek", "position_ms": position_ms});
  let resp = send_ipc_command(&req)?;
  println!("{}", resp);
  Ok(())
}

fn cmd_stop() -> Result<(), Box<dyn std::error::Error>> {
  let req = serde_json::json!({"command": "stop"});
  let resp = send_ipc_command(&req)?;
  println!("{}", resp);
  Ok(())
}

fn cmd_status() -> Result<(), Box<dyn std::error::Error>> {
  let req = serde_json::json!({"command": "status"});
  let resp = send_ipc_command(&req)?;
  println!("{}", serde_json::to_string_pretty(&resp)?);
  Ok(())
}

fn cmd_load_decoder(path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
  let req = serde_json::json!({"command": "load_decoder", "path": path.display().to_string()});
  let resp = send_ipc_command(&req)?;
  if resp["status"] == "ok" {
    println!("decoder loaded: {}", serde_json::to_string_pretty(&resp)?);
  } else {
    eprintln!("error: {} - {}", resp["code"], resp["message"]);
    std::process::exit(1);
  }
  Ok(())
}

fn cmd_list_decoders() -> Result<(), Box<dyn std::error::Error>> {
  let req = serde_json::json!({"command": "list_decoders"});
  let resp = send_ipc_command(&req)?;
  if resp["status"] == "ok" {
    if let Some(decoders) = resp.get("decoders") {
      println!("{}", serde_json::to_string_pretty(decoders)?);
    }
  } else {
    eprintln!("error: {} - {}", resp["code"], resp["message"]);
    std::process::exit(1);
  }
  Ok(())
}

fn cmd_cleanup() -> Result<(), Box<dyn std::error::Error>> {
  let req = serde_json::json!({"command": "cleanup"});
  let resp = send_ipc_command(&req)?;
  if resp["status"] == "ok" {
    if let Some(msg) = resp.get("message") {
      let parsed: serde_json::Value = serde_json::from_str(msg.as_str().unwrap_or("{}"))?;
      if let Some(files) = parsed.get("removed_files").and_then(|v| v.as_array()) {
        if files.is_empty() {
          println!("no stale files found");
        } else {
          println!("removed {} stale file(s):", files.len());
          for f in files {
            println!("  {}", f.as_str().unwrap_or("?"));
          }
        }
      }
    }
  } else {
    eprintln!("error: {} - {}", resp["code"], resp["message"]);
    std::process::exit(1);
  }
  Ok(())
}
