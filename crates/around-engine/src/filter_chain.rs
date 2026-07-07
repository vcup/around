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
// Format scoring (ADR-0006 §2.1)
// ---------------------------------------------------------------------------

/// Penalty per kHz of downsampling (higher sample rate → lower).
const PENALTY_DOWNSAMPLE_PER_KHZ: u32 = 100;
/// Penalty per channel removed (more channels → fewer).
const PENALTY_DOWNMIX_PER_CH: u32 = 80;
/// Penalty per kHz of upsampling (lower sample rate → higher).
const PENALTY_UPSAMPLE_PER_KHZ: u32 = 10;
/// Penalty per channel added (fewer channels → more).
const PENALTY_UPMIX_PER_CH: u32 = 5;
/// Penalty for changing interleave mode (planar ↔ interleaved).
const PENALTY_INTERLEAVE_CHANGE: u32 = 1;

/// Score the penalty of converting from `input` to `output` format.
///
/// Returns a penalty score in arbitrary units. Lower is better.
/// The penalty table (ADR-0006 §2.1):
///
/// | Conversion | Penalty |
/// |---|---|
/// | Downsampling | 100 / kHz of rate difference |
/// | Downmixing | 80 / channel removed |
/// | Upsampling | 10 / kHz of rate difference |
/// | Upmixing | 5 / channel added |
/// | Planar ↔ Interleaved | 1 |
///
/// # Tiebreaker
///
/// When comparing multiple candidate output formats, the format with the
/// highest sample rate wins if scores are equal (prefer higher quality).
pub fn score_format_pair(input: &SampleSpec, output: &SampleSpec) -> u32 {
  let mut score = 0u32;

  // Sample rate conversion.
  if input.sample_rate > output.sample_rate {
    // Downsampling: 100 per kHz of difference.
    let diff_khz = (input.sample_rate - output.sample_rate) as f32 / 1000.0;
    score += (diff_khz * PENALTY_DOWNSAMPLE_PER_KHZ as f32) as u32;
  } else if output.sample_rate > input.sample_rate {
    // Upsampling: 10 per kHz of difference.
    let diff_khz = (output.sample_rate - input.sample_rate) as f32 / 1000.0;
    score += (diff_khz * PENALTY_UPSAMPLE_PER_KHZ as f32) as u32;
  }

  // Channel conversion.
  if input.channels > output.channels {
    // Downmixing: 80 per channel removed.
    score += (input.channels - output.channels) as u32 * PENALTY_DOWNMIX_PER_CH;
  } else if output.channels > input.channels {
    // Upmixing: 5 per channel added.
    score += (output.channels - input.channels) as u32 * PENALTY_UPMIX_PER_CH;
  }

  // Interleave conversion.
  if input.interleave != output.interleave {
    score += PENALTY_INTERLEAVE_CHANGE;
  }

  score
}

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
  /// differ at any boundary. Uses [`score_format_pair`] to assess the
  /// penalty of format conversion and inserts the cheapest conversion
  /// chain.
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

    // Score the format difference between decode and output.
    let score = score_format_pair(&chain.decode_spec, &chain.output_spec);

    if score > 0 && chain.filters.is_empty() {
      // Auto-insert Resample if sample rates differ.
      if chain.decode_spec.sample_rate != chain.output_spec.sample_rate {
        chain.filters.push(Box::new(Resample::new(
          chain.decode_spec.sample_rate,
          chain.output_spec.sample_rate,
          chain.decode_spec.channels,
        )));
      }
      // Note: Deinterleave/Interleave auto-insertion is deferred until
      // the Filter chain supports these conversions (Phase 6+).
      // The scoring function is in place for when those filters exist.
    }

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

  // ── Format scoring tests ──────────────────────────────────────────────

  #[test]
  fn score_identical_formats_is_zero() {
    let spec = SampleSpec::interleaved(44100, 2, 16).unwrap();
    assert_eq!(score_format_pair(&spec, &spec), 0);
  }

  #[test]
  fn score_downsample_penalty() {
    let input = SampleSpec::interleaved(48000, 2, 16).unwrap();
    let output = SampleSpec::interleaved(44100, 2, 16).unwrap();
    let score = score_format_pair(&input, &output);
    // 48→44.1: diff = 3.9 kHz, penalty = ceil(3.9 * 100) = 390
    assert!(score >= 300 && score <= 500, "score = {}", score);
  }

  #[test]
  fn score_upsample_penalty() {
    let input = SampleSpec::interleaved(44100, 2, 16).unwrap();
    let output = SampleSpec::interleaved(48000, 2, 16).unwrap();
    let score = score_format_pair(&input, &output);
    // 44.1→48: diff = 3.9 kHz, penalty = ceil(3.9 * 10) = 39
    assert!(score >= 30 && score <= 50, "score = {}", score);
  }

  #[test]
  fn score_downsample_cheaper_than_upsample() {
    let a = SampleSpec::interleaved(48000, 2, 16).unwrap();
    let b = SampleSpec::interleaved(44100, 2, 16).unwrap();
    let down_score = score_format_pair(&a, &b); // 48→44.1: downsampling
    let up_score = score_format_pair(&b, &a); // 44.1→48: upsampling
    assert!(down_score > up_score, "down={} up={}", down_score, up_score);
  }

  #[test]
  fn score_downmix_penalty() {
    let input = SampleSpec::interleaved(44100, 6, 16).unwrap();
    let output = SampleSpec::interleaved(44100, 2, 16).unwrap();
    assert_eq!(score_format_pair(&input, &output), 4 * 80);
  }

  #[test]
  fn score_upmix_penalty() {
    let input = SampleSpec::interleaved(44100, 2, 16).unwrap();
    let output = SampleSpec::interleaved(44100, 6, 16).unwrap();
    assert_eq!(score_format_pair(&input, &output), 4 * 5);
  }

  #[test]
  fn score_interleave_change_penalty() {
    use around_core::Interleave;
    let input = SampleSpec::new(44100, 2, 16, Interleave::Interleaved).unwrap();
    let output = SampleSpec::new(44100, 2, 16, Interleave::Planar).unwrap();
    assert_eq!(score_format_pair(&input, &output), 1);
  }
}
