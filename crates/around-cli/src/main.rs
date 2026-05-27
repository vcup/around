//! around: A cross-platform audio player — CLI entry point.

use around_core::Source;
use around_engine::{Engine, EngineConfig};
use around_source_file::FileSource;
use clap::{Parser, Subcommand};
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
