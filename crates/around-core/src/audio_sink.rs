//! Audio output sink abstraction.
//!
//! Defines [`AudioSink`] — the contract for sending decoded PCM samples
//! to an output destination. Three implementations are provided:
//!
//! * [`NullSink`] — silent discard (production, always available)
//! * [`RingBufSink`] — in-memory capture for tests (`feature = "test-utils"`)
//! * [`CpalSink`] — real audio device via cpal (in `around-engine::cpal_sink`)
//!
//! # Architecture
//!
//! The decode loop in `Engine::run_stream_with_sink()` calls `write()` once
//! per decode iteration with a full interleaved `f32` buffer. The sink must
//! consume or forward all samples synchronously — partial writes are not
//! supported by the trait contract.
//!
//! # Thread safety
//!
//! `AudioSink: Send + Sync` — the sink may be shared across threads
//! (e.g. via `Arc<Engine>`) and used exclusively from the decode thread.
//! `Sync` is required for the engine to be `Send` behind `Arc`.

use crate::AroundError;
#[cfg(feature = "test-utils")]
use std::sync::Arc;

// ---------------------------------------------------------------------------
// AudioSink trait
// ---------------------------------------------------------------------------

/// Abstract audio output for the decode loop.
///
/// Implementations map PCM sample blocks to either a real audio device
/// ([`CpalSink`]), a silent discard ([`NullSink`]), or a test capture buffer
/// ([`RingBufSink`]).
///
/// # Contract
///
/// * `write()` consumes all samples synchronously. Returning an error is
///   fatal for the stream — the decode loop stops.
/// * `sample_rate()` and `channels()` must be constant for the lifetime of
///   the sink (the decode loop calls them once at construction).
///
pub trait AudioSink: Send + Sync {
  /// Write a block of interleaved `f32` PCM samples.
  ///
  /// `samples` contains `channels * num_frames` values in interleaved
  /// order (L, R, L, R, … for stereo).
  ///
  /// Returns `Ok(())` on success. Returns `Err(AroundError)` if the output
  /// cannot accept more data — the stream will be stopped.
  fn write(&mut self, samples: &[f32]) -> Result<(), AroundError>;

  /// Sample rate in Hz expected by this sink (e.g. `44100`).
  fn sample_rate(&self) -> u32;

  fn channels(&self) -> u8;
}

// ---------------------------------------------------------------------------
// NullSink — silent discard
// ---------------------------------------------------------------------------

/// Silent sink: consumes samples without producing audio.
///
/// Useful for headless CI, benchmarking, or streams that only need position
/// tracking without audible output.
///
/// # Production use
///
/// `NullSink` is **not** a test-only type. It is unconditionally compiled
/// and available for production headless/automation scenarios.
pub struct NullSink {
  sample_rate: u32,
  channels: u8,
}

impl NullSink {
  pub fn new(sample_rate: u32, channels: u8) -> Self {
    Self {
      sample_rate,
      channels,
    }
  }
}

impl AudioSink for NullSink {
  fn write(&mut self, _samples: &[f32]) -> Result<(), AroundError> {
    Ok(()) // discard — no-op
  }

  fn sample_rate(&self) -> u32 {
    self.sample_rate
  }

  fn channels(&self) -> u8 {
    self.channels
  }
}

// ---------------------------------------------------------------------------
// RingBufSink — test capture buffer
// ---------------------------------------------------------------------------

/// Capture sink: accumulates all written samples into a `Vec<f32>` for test
/// assertions.
///
/// The buffer is `Arc<parking_lot::Mutex<…>>` so the test can share a
/// reference with the decode thread without lifetime fights. This is a
/// **test-only** type, enabled behind `#[cfg(feature = "test-utils")]`.
///
/// # Cross-crate access
///
/// Tests in `around-engine` or other crates enable this type via:
/// ```toml
/// [dev-dependencies]
/// around-core = { path = "../around-core", features = ["test-utils"] }
/// ```
#[cfg(feature = "test-utils")]
pub struct RingBufSink {
  buffer: Arc<parking_lot::Mutex<Vec<f32>>>,
  sample_rate: u32,
  channels: u8,
}

#[cfg(feature = "test-utils")]
impl RingBufSink {
  /// Create a new capture sink with the given stream parameters.
  pub fn new(sample_rate: u32, channels: u8) -> Self {
    Self {
      buffer: Arc::new(parking_lot::Mutex::new(Vec::new())),
      sample_rate,
      channels,
    }
  }

  /// Return a snapshot of all samples written so far.
  pub fn samples(&self) -> Vec<f32> {
    self.buffer.lock().clone()
  }

  /// Number of samples captured so far.
  pub fn len(&self) -> usize {
    self.buffer.lock().len()
  }

  /// Returns true if no samples have been captured.
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  /// Return a `RingBufSinkHandle` for shared access across threads.
  ///
  /// The handle is `Clone` — clone it before starting the decode thread,
  /// then inspect captured samples after the thread completes.
  pub fn handle(&self) -> RingBufSinkHandle {
    RingBufSinkHandle {
      buffer: Arc::clone(&self.buffer),
    }
  }
}

#[cfg(feature = "test-utils")]
impl AudioSink for RingBufSink {
  fn write(&mut self, samples: &[f32]) -> Result<(), AroundError> {
    self.buffer.lock().extend_from_slice(samples);
    Ok(())
  }

  fn sample_rate(&self) -> u32 {
    self.sample_rate
  }

  fn channels(&self) -> u8 {
    self.channels
  }
}

/// A handle to a [`RingBufSink`]'s buffer, usable across threads.
///
/// Tests clone this before starting the decode thread, then inspect after
/// the thread completes.
#[cfg(feature = "test-utils")]
#[derive(Clone)]
pub struct RingBufSinkHandle {
  buffer: Arc<parking_lot::Mutex<Vec<f32>>>,
}

#[cfg(feature = "test-utils")]
impl RingBufSinkHandle {
  /// Return a snapshot of all samples captured so far.
  pub fn samples(&self) -> Vec<f32> {
    self.buffer.lock().clone()
  }

  /// Number of samples captured so far.
  pub fn len(&self) -> usize {
    self.buffer.lock().len()
  }

  /// Returns true if no samples have been captured.
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }
}

#[cfg(feature = "test-utils")]
impl From<RingBufSink> for RingBufSinkHandle {
  fn from(sink: RingBufSink) -> Self {
    sink.handle()
  }
}
