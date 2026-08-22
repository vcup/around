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
  resampler: Option<LinearResampler>,
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
    let default_config = device
      .default_output_config()
      .map_err(|e| AroundError::Internal {
        message: format!("audio device error: {e}"),
      })?;
    let supported = device
      .supported_output_configs()
      .map_err(|e| AroundError::Internal {
        message: format!("audio device config error: {e}"),
      })?
      .collect::<Vec<_>>();
    let f32_configs = supported
      .iter()
      .copied()
      .filter(|range| range.sample_format() == cpal::SampleFormat::F32);

    // Prefer the source format when the device accepts it. Otherwise use the
    // device's native f32 format and resample in the sink.
    let config = f32_configs
      .filter(|range| range.channels() == u16::from(channels))
      .find_map(|range| range.try_with_sample_rate(cpal::SampleRate(sample_rate)))
      .or_else(|| {
        supported
          .iter()
          .copied()
          .filter(|range| {
            range.channels() == default_config.channels()
              && range.sample_format() == cpal::SampleFormat::F32
          })
          .find_map(|range| range.try_with_sample_rate(default_config.sample_rate()))
      })
      .ok_or_else(|| AroundError::Internal {
        message: format!(
          "audio device has no supported f32 output format for source {sample_rate} Hz/{channels} channels"
        ),
      })?;
    let output_sample_rate = config.sample_rate().0;
    let output_channels = u8::try_from(config.channels()).map_err(|_| AroundError::Internal {
      message: format!(
        "audio device has invalid channel count {}",
        config.channels()
      ),
    })?;
    let stream_config = config.config();
    tracing::debug!(
      source_sample_rate = sample_rate,
      source_channels = channels,
      output_sample_rate,
      output_channels,
      sample_format = ?config.sample_format(),
      "selected audio output format"
    );

    let (producer, consumer) = create_output(16384);
    let consumer = Arc::new(Mutex::new(consumer));
    let consumer_clone = Arc::clone(&consumer);
    let device_lost = Arc::new(AtomicBool::new(false));
    let device_lost_clone = Arc::clone(&device_lost);

    let stream = device
      .build_output_stream(
        &stream_config,
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
      sample_rate: output_sample_rate,
      channels: output_channels,
      device_lost,
      resampler: LinearResampler::new(sample_rate, channels, output_sample_rate, output_channels),
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

/// Stateful linear resampler used when the device's native format differs
/// from the source format. One input frame is retained across writes so chunk
/// boundaries do not reset interpolation state.
struct LinearResampler {
  input_channels: usize,
  output_channels: usize,
  step: f64,
  position: f64,
  pending: Vec<f32>,
  output: Vec<f32>,
}

impl LinearResampler {
  fn new(
    input_rate: u32,
    input_channels: u8,
    output_rate: u32,
    output_channels: u8,
  ) -> Option<Self> {
    if input_rate == output_rate && input_channels == output_channels {
      return None;
    }
    Some(Self {
      input_channels: usize::from(input_channels),
      output_channels: usize::from(output_channels),
      step: f64::from(input_rate) / f64::from(output_rate),
      position: 0.0,
      pending: Vec::new(),
      output: Vec::new(),
    })
  }

  fn process(&mut self, samples: &[f32]) -> Result<(), AroundError> {
    if samples.len() % self.input_channels != 0 {
      return Err(AroundError::Internal {
        message: format!(
          "resampler received {} samples for {} channels",
          samples.len(),
          self.input_channels
        ),
      });
    }
    self.pending.extend_from_slice(samples);
    self.output.clear();

    let input_frames = self.pending.len() / self.input_channels;
    if input_frames < 2 {
      return Ok(());
    }
    let estimated_frames = ((input_frames as f64 - self.position) / self.step).ceil() as usize;
    self
      .output
      .reserve(estimated_frames.saturating_mul(self.output_channels));

    while self.position + 1.0 < input_frames as f64 {
      let source_frame = self.position.floor() as usize;
      let fraction = (self.position - source_frame as f64) as f32;
      let pending = &self.pending;
      let input_channels = self.input_channels;
      let interpolate = |channel: usize| {
        let a = pending[source_frame * input_channels + channel];
        let b = pending[(source_frame + 1) * input_channels + channel];
        a + (b - a) * fraction
      };

      if self.output_channels == 1 {
        let sum: f32 = (0..input_channels).map(interpolate).sum();
        self.output.push(sum / input_channels as f32);
      } else if input_channels == 1 {
        let value = interpolate(0);
        for _ in 0..self.output_channels {
          self.output.push(value);
        }
      } else {
        for channel in 0..self.output_channels {
          self
            .output
            .push(interpolate(channel.min(input_channels - 1)));
        }
      }
      self.position += self.step;
    }

    let consumed_frames = (self.position.floor() as usize).min(input_frames - 1);
    if consumed_frames > 0 {
      let consumed_samples = consumed_frames * self.input_channels;
      let remaining_samples = self.pending.len() - consumed_samples;
      self.pending.copy_within(consumed_samples.., 0);
      self.pending.truncate(remaining_samples);
      self.position -= consumed_frames as f64;
    }
    Ok(())
  }
}

impl AudioSink for CpalSink {
  fn write(&mut self, samples: &[f32]) -> Result<(), AroundError> {
    if let Some(resampler) = self.resampler.as_mut() {
      resampler.process(samples)?;
    }

    let output = self
      .resampler
      .as_ref()
      .map_or(samples, |resampler| resampler.output.as_slice());
    let mut offset = 0;
    while offset < output.len() {
      if self.device_lost.load(Ordering::SeqCst) {
        return Err(AroundError::Internal {
          message: "audio output device lost while buffering samples".into(),
        });
      }

      let written = self.producer.push_slice(&output[offset..]);
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
#[cfg(test)]
mod tests {
  use super::LinearResampler;

  #[test]
  fn resampler_preserves_duration_across_chunks() {
    let Some(mut resampler) = LinearResampler::new(44_100, 2, 48_000, 2) else {
      unreachable!("different sample rates must create a resampler");
    };
    let input = vec![0.0f32; 44_100 * 2];
    let mut output_samples = 0;
    for chunk in input.chunks(997 * 2) {
      assert!(resampler.process(chunk).is_ok());
      output_samples += resampler.output.len();
    }

    let output_frames = output_samples / 2;
    assert!((47_990..=48_010).contains(&output_frames));
  }

  #[test]
  fn resampler_is_not_used_for_matching_format() {
    assert!(LinearResampler::new(48_000, 2, 48_000, 2).is_none());
  }
}
