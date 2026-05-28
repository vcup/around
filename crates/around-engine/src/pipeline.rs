//! Audio pipeline: Source → Decoder → Output Sink.

use crate::config::{select_decoder, try_open_decoder, EngineConfig};
use around_core::{AroundError, Metadata, SampleSpec, Source};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The audio playback engine.
pub struct Engine {
  config: EngineConfig,
  running: Arc<AtomicBool>,
  device_lost: Arc<AtomicBool>,
}

/// Handle for controlling an active playback session.
pub struct PlaybackHandle {
  pub output_format: SampleSpec,
  pub metadata: Metadata,
  running: Arc<AtomicBool>,
}

impl PlaybackHandle {
  /// Signal playback to stop at the next opportunity.
  pub fn stop(&self) {
    self.running.store(false, Ordering::SeqCst);
  }

  /// Check whether playback is still active.
  pub fn is_running(&self) -> bool {
    self.running.load(Ordering::SeqCst)
  }
}

impl Engine {
  /// Create a new engine with the given configuration.
  pub fn new(config: EngineConfig) -> Self {
    Self {
      config,
      device_lost: Arc::new(AtomicBool::new(false)),
      running: Arc::new(AtomicBool::new(false)),
    }
  }

  /// Play a source. Blocks until playback completes or [`PlaybackHandle::stop`] is called.
  pub fn play(&self, source: Box<dyn Source>) -> Result<PlaybackHandle, AroundError> {
    let decoder_name = select_decoder(source.as_ref())?;

    let (mut decoder, output_format, metadata) = try_open_decoder(source, decoder_name)?;

    let running = Arc::clone(&self.running);
    running.store(true, Ordering::SeqCst);

    let handle = PlaybackHandle {
      output_format,
      metadata,
      running: Arc::clone(&self.running),
    };

    let _span = tracing::info_span!("playback").entered();
    tracing::info!("starting playback");

    let mut buf = vec![0.0f32; 4096];
    let mut total_samples = 0usize;
    let mut consecutive_errors = 0u32;
    while running.load(Ordering::SeqCst) {
      if self.device_lost.load(Ordering::SeqCst) {
        tracing::warn!("playback paused: audio device lost");
        if self.config.output_auto_reconnect {
          self.device_lost.store(false, Ordering::SeqCst);
          continue;
        } else {
          break;
        }
      }
      match decoder.read(&mut buf) {
        Ok(Some(n)) => {
          total_samples += n;
          consecutive_errors = 0;
        }
        Ok(None) => break,
        Err(e) => {
          consecutive_errors += 1;
          tracing::warn!(?e, consecutive_errors, "decode error during playback");
          if consecutive_errors >= 3 {
            tracing::error!(?e, "too many consecutive decode errors, stopping");
            return Err(e);
          }
        }
      }
    }

    tracing::info!(total_samples, "playback complete");
    running.store(false, Ordering::SeqCst);

    Ok(handle)
  }

  /// Return a reference to the engine's configuration.
  /// Signal the engine to stop playback.
  pub fn stop(&self) {
    self.running.store(false, Ordering::SeqCst);
  }

  /// Notify the engine that the audio output device was disconnected.
  /// This triggers auto-pause behavior per config.
  pub fn notify_device_lost(&self) {
    self.device_lost.store(true, Ordering::SeqCst);
    tracing::warn!("audio output device disconnected");
    if !self.config.output_auto_reconnect {
      self.stop();
    }
  }
  pub fn config(&self) -> &EngineConfig {
    &self.config
  }
}
