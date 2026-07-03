//! Filter trait and FilterChain for audio processing (ADR-0006).
//!
//! Filters are ordered in a chain: each filter receives samples, transforms
//! them, and passes them to the next. Format negotiation auto-inserts
//! Resample, Deinterleave, and Interleave at format boundaries.
//!
//! # Architecture
//!
//! - [`Filter`] — trait implemented by all audio filters.
//! - [`FilterInfo`] — metadata: name, supported input/output formats.
//! - [`FilterChain`] — ordered list with format negotiation via `build()`.
//! - [`Resample`] — sample rate conversion filter.
//! - [`Volume`] — gain multiplier filter.

use around_core::SampleSpec;

// ---------------------------------------------------------------------------
// Filter trait
// ---------------------------------------------------------------------------

/// Audio processing filter.
///
/// Filters are stateless in their configuration but may hold per-stream
/// state. `process()` receives interleaved f32 PCM in-place.
pub trait Filter: Send + Sync {
  /// Process audio samples in-place.
  /// `buf` contains interleaved f32 PCM.
  /// `channels` is the number of channels.
  /// Returns the number of samples produced (may differ from input after resampling).
  fn process(&mut self, buf: &mut [f32], channels: u8) -> usize;

  /// Return metadata about this filter.
  fn info(&self) -> FilterInfo;

  /// Number of channels this filter requires/produces.
  /// Default: passes through channels unchanged.
  fn channels_in(&self) -> Option<u8> {
    None
  }
  fn channels_out(&self) -> Option<u8> {
    None
  }
}

/// Metadata for a filter.
#[derive(Debug, Clone)]
pub struct FilterInfo {
  /// Human-readable filter name.
  pub name: String,
  /// Accepted input formats (empty = any).
  pub formats_in: Vec<SampleSpec>,
  /// Produced output formats (empty = same as input).
  pub formats_out: Vec<SampleSpec>,
}

// ---------------------------------------------------------------------------
// FilterChain
// ---------------------------------------------------------------------------

/// Ordered list of audio filters with format negotiation.
#[allow(dead_code)]
pub struct FilterChain {
  filters: Vec<Box<dyn Filter>>,
  /// Decoded format from the codec.
  decode_spec: SampleSpec,
  /// Target output format.
  output_spec: SampleSpec,
}

impl FilterChain {
  /// Build a filter chain from user-selected filters.
  ///
  /// Automatically inserts resampling filters when input/output formats
  /// differ at any boundary. For now, applies a simplified penalty model:
  /// prefers fewer conversions.
  pub fn build(
    decode_spec: SampleSpec,
    filters: Vec<Box<dyn Filter>>,
    output_spec: SampleSpec,
  ) -> Self {
    let mut chain = FilterChain {
      filters,
      decode_spec,
      output_spec,
    };

    // Auto-insert resample if decode format ≠ output format and chain is empty.
    if chain.filters.is_empty() && decode_spec.sample_rate != output_spec.sample_rate {
      chain.filters.push(Box::new(Resample::new(
        decode_spec.sample_rate,
        output_spec.sample_rate,
        decode_spec.channels,
      )));
    }

    // For each adjacent pair, check format compatibility.
    // In a full implementation (Phase 6+), this would negotiate
    // interleave mode and insert Deinterleave/Interleave filters.
    // For now, we assume interleaved throughout.

    chain
  }

  /// Process samples through the entire filter chain.
  /// Returns the number of samples produced.
  pub fn process(&mut self, buf: &mut [f32], channels: u8) -> usize {
    let mut len = buf.len();
    for filter in &mut self.filters {
      len = filter.process(&mut buf[..len], channels);
      if len == 0 {
        break;
      }
    }
    len
  }

  /// Whether the chain is empty (passthrough).
  pub fn is_empty(&self) -> bool {
    self.filters.is_empty()
  }

  /// Number of filters in the chain.
  pub fn len(&self) -> usize {
    self.filters.len()
  }
}

// ---------------------------------------------------------------------------
// Built-in: Resample filter
// ---------------------------------------------------------------------------

/// Sample rate conversion filter.
///
/// Uses a simple linear interpolation resampler. For production use,
/// replace with `rubato`-based high-quality resampling.
#[allow(dead_code)]
pub struct Resample {
  sample_rate_in: u32,
  sample_rate_out: u32,
  ratio: f64,
  channels: u8,
  /// Accumulator for fractional sample position.
  accum: f64,
}

impl Resample {
  pub fn new(sample_rate_in: u32, sample_rate_out: u32, channels: u8) -> Self {
    Self {
      sample_rate_in,
      sample_rate_out,
      ratio: sample_rate_in as f64 / sample_rate_out as f64,
      channels,
      accum: 0.0,
    }
  }
}

impl Filter for Resample {
  fn process(&mut self, buf: &mut [f32], _channels: u8) -> usize {
    let ch = self.channels as usize;
    if ch == 0 || self.ratio <= 0.0 {
      return buf.len();
    }

    let input_frames = buf.len() / ch;
    let output_frames = (input_frames as f64 / self.ratio).ceil() as usize;
    let mut out = vec![0.0f32; output_frames * ch];
    let mut out_pos = 0;

    for out_frame in 0..output_frames {
      let src_pos = out_frame as f64 * self.ratio;
      let src_frame = src_pos as usize;
      let frac = src_pos - src_frame as f64;

      if src_frame + 1 < input_frames {
        for c in 0..ch {
          let a = buf[src_frame * ch + c];
          let b = buf[(src_frame + 1) * ch + c];
          out[out_pos + c] = a as f64 as f32 + ((b - a) as f64 * frac) as f32;
        }
      } else if src_frame < input_frames {
        for c in 0..ch {
          out[out_pos + c] = buf[src_frame * ch + c];
        }
      }
      out_pos += ch;
    }

    let produced = out_pos.min(buf.len());
    buf[..produced].copy_from_slice(&out[..produced]);
    produced
  }

  fn info(&self) -> FilterInfo {
    FilterInfo {
      name: "resample".into(),
      formats_in: vec![],
      formats_out: vec![],
    }
  }

  fn channels_in(&self) -> Option<u8> {
    Some(self.channels)
  }
  fn channels_out(&self) -> Option<u8> {
    Some(self.channels)
  }
}

// ---------------------------------------------------------------------------
// Built-in: Volume filter
// ---------------------------------------------------------------------------

/// Simple gain multiplier filter.
pub struct Volume {
  gain: f32,
}

impl Volume {
  pub fn new(gain_db: f32) -> Self {
    Self {
      gain: 10.0f32.powf(gain_db / 20.0),
    }
  }

  pub fn with_gain(gain: f32) -> Self {
    Self { gain }
  }
}

impl Filter for Volume {
  fn process(&mut self, buf: &mut [f32], _channels: u8) -> usize {
    for sample in buf.iter_mut() {
      *sample *= self.gain;
    }
    buf.len()
  }

  fn info(&self) -> FilterInfo {
    FilterInfo {
      name: format!("volume({:.1}dB)", 20.0 * self.gain.log10()),
      formats_in: vec![],
      formats_out: vec![],
    }
  }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn empty_chain_passthrough() {
    let spec = SampleSpec::interleaved(44100, 2, 16).unwrap();
    let mut chain = FilterChain::build(spec, vec![], spec);
    assert!(chain.is_empty());
    let mut buf = vec![0.5f32; 100];
    let len = chain.process(&mut buf, 2);
    assert_eq!(len, 100);
  }

  #[test]
  fn volume_attenuates() {
    let mut vol = Volume::with_gain(0.5);
    let mut buf = vec![1.0f32; 10];
    vol.process(&mut buf, 1);
    assert!((buf[0] - 0.5).abs() < 0.001);
  }

  #[test]
  fn resample_downsample_preserves_length() {
    let mut r = Resample::new(48000, 24000, 1);
    let mut buf: Vec<f32> = (0..480).map(|i| (i as f32).sin()).collect();
    let produced = r.process(&mut buf, 1);
    // Downsampling by 2x should produce ~240 samples.
    assert!(
      produced >= 200 && produced <= 280,
      "expected ~240 samples, got {}",
      produced
    );
  }

  #[test]
  fn resample_upsample_increases_length() {
    let mut r = Resample::new(24000, 48000, 1);
    // Upsampling: 240 input frames at ratio 0.5 → ~480 output frames.
    // Buffer must be sized for output (algorithm uses full buffer as input).
    let mut buf: Vec<f32> = (0..240).map(|i| (i as f32).sin()).collect();
    // Allocate extra space for upsampled output.
    buf.resize(500, 0.0);
    let produced = r.process(&mut buf, 1);
    assert!(
      produced > 240 && produced <= 500,
      "expected >240 and <=500 samples, got {}",
      produced
    );
  }

  #[test]
  fn filter_chain_with_volume() {
    let spec = SampleSpec::interleaved(44100, 2, 16).unwrap();
    let filters: Vec<Box<dyn Filter>> = vec![Box::new(Volume::with_gain(0.5))];
    let mut chain = FilterChain::build(spec, filters, spec);
    assert_eq!(chain.len(), 1);
    let mut buf = vec![1.0f32; 20];
    chain.process(&mut buf, 2);
    assert!((buf[0] - 0.5).abs() < 0.001);
  }
}
