//! Real audio output via cpal.
//!
//! Internal architecture:
//!   Decode thread → CpalSink::write → AudioProducer → ringbuf → AudioConsumer → cpal callback → device
//!
//! This is exactly the device setup that was previously inline in `run_stream()`,
//! now encapsulated in a reusable type.

use crate::output::{create_output, AudioProducer};
use around_core::audio_sink::AudioSink;
use around_core::AroundError;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Real audio device sink. Holds the write side of a ring buffer and the
/// cpal output stream that drains the read side on the audio callback thread.
///
/// The cpal stream is started in `new()` and kept alive for the lifetime of
/// the sink. Dropping the sink stops the stream automatically.
pub struct CpalSink {
  producer: AudioProducer,
  _stream: cpal::Stream, // kept alive; drop = stop audio
  sample_rate: u32,
  channels: u8,
  device_lost: Arc<AtomicBool>,
}

impl CpalSink {
  /// Create a new `CpalSink`, opening the default output device.
  ///
  /// Returns `Err(AroundError::Internal)` if no output device is available
  /// or the stream cannot be built — matching the current error semantics
  /// of `run_stream()`.
  pub fn new(sample_rate: u32, channels: u8) -> Result<Self, AroundError> {
    let host = cpal::default_host();
    let device = host
      .default_output_device()
      .ok_or_else(|| AroundError::Internal {
        message: "no audio output device found".into(),
      })?;
    let config = device
      .default_output_config()
      .map_err(|e| AroundError::Internal {
        message: format!("audio device error: {e}"),
      })?;

    let (producer, consumer) = create_output(16384);
    let consumer = Arc::new(Mutex::new(consumer));
    let consumer_clone = Arc::clone(&consumer);
    let device_lost = Arc::new(AtomicBool::new(false));
    let device_lost_clone = Arc::clone(&device_lost);

    let stream = device
      .build_output_stream(
        &config.into(),
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
          let mut cons = consumer_clone.lock();
          let popped = cons.pop_slice(data);
          if popped < data.len() {
            data[popped..].fill(0.0);
          }
        },
        move |err| {
          tracing::error!(?err, "cpal stream error");
          device_lost_clone.store(true, Ordering::SeqCst);
        },
        None,
      )
      .map_err(|e| AroundError::Internal {
        message: format!("failed to build audio output stream: {e}"),
      })?;

    stream.play().map_err(|e| AroundError::Internal {
      message: format!("failed to start audio output stream: {e}"),
    })?;

    Ok(Self {
      producer,
      _stream: stream,
      sample_rate,
      channels,
      device_lost,
    })
  }

  /// Check whether the audio device has signalled a loss/error.
  ///
  /// The decode loop checks this on each iteration and triggers the
  /// device-lost / auto-reconnect logic in `Engine`.
  pub fn is_device_lost(&self) -> bool {
    self.device_lost.load(Ordering::SeqCst)
  }
}

impl AudioSink for CpalSink {
  fn write(&mut self, samples: &[f32]) -> Result<(), AroundError> {
    let mut offset = 0;
    while offset < samples.len() {
      if self.device_lost.load(Ordering::SeqCst) {
        return Err(AroundError::Internal {
          message: "audio output device lost while buffering samples".into(),
        });
      }

      let written = self.producer.push_slice(&samples[offset..]);
      offset += written;
      if written == 0 {
        // Backpressure is required here: decoding faster than the CPAL
        // callback must not advance playback time while dropping PCM.
        std::thread::sleep(std::time::Duration::from_millis(1));
      }
    }
    Ok(())
  }

  fn sample_rate(&self) -> u32 {
    self.sample_rate
  }

  fn channels(&self) -> u8 {
    self.channels
  }
}

// SAFETY: CpalSink is Send because:
// - AudioProducer (HeapProd<f32>) is Send (SPSC producer)
// - cpal::Stream is Send + Sync (handle; audio thread managed by OS)

// SAFETY: CpalSink is Send + Sync because:
// - AudioProducer (HeapProd<f32>) is Send + Sync (SPSC producer)
// - cpal::Stream is NOT Send/Sync on Linux due to PhantomData<*mut ()>,
//   but the stream handle is actually thread-safe — it only sends data
//   to the OS audio subsystem.
// - Arc<AtomicBool> is Send + Sync
// The CpalSink is created and consumed on the decode thread.
unsafe impl Send for CpalSink {}
unsafe impl Sync for CpalSink {}
