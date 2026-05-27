//! around: A cross-platform audio player — CLI entry point.

use around_core::Source;
use around_engine::{Engine, EngineConfig};
use around_source_file::FileSource;
use clap::{Parser, Subcommand};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
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
  }
}

fn cmd_play(path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
  let config = EngineConfig::load();
  let engine = Arc::new(Engine::new(config));

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

  tracing::info!("playing '{}'", path.display());

  // Run play() on a background thread — it blocks until stop() or natural completion.
  let eng = engine.clone();
  let play_thread = thread::spawn(move || eng.play(source_box));

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
      return Err(Box::new(e));
    }
    Err(_panic) => {
      return Err("playback thread panicked".into());
    }
  }

  Ok(())
}

/// Send a JSON command to the engine via Unix socket and read the response.
fn send_ipc_command(
  req: &serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
  let mut stream = UnixStream::connect("/tmp/around.sock")?;

  let request = req.to_string();
  stream.write_all(request.as_bytes())?;
  stream.write_all(b"\n")?;

  let mut buf = String::new();
  stream.read_to_string(&mut buf)?;

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
