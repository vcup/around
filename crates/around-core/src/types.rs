//! Core type aliases and constants for audio primitives.

use std::fmt;

// --- BitDepth ---

pub type BitDepth = u8;

pub const BIT_DEPTH_8: BitDepth = 8;
pub const BIT_DEPTH_16: BitDepth = 16;
pub const BIT_DEPTH_24: BitDepth = 24;
pub const BIT_DEPTH_32: BitDepth = 32;
pub const BIT_DEPTH_64: BitDepth = 64;

// --- SampleRate ---

pub type SampleRate = u32;

pub const SAMPLE_RATE_44100: SampleRate = 44100;
pub const SAMPLE_RATE_48000: SampleRate = 48000;
pub const SAMPLE_RATE_88200: SampleRate = 88200;
pub const SAMPLE_RATE_96000: SampleRate = 96000;
pub const SAMPLE_RATE_176400: SampleRate = 176400;
pub const SAMPLE_RATE_192000: SampleRate = 192000;

// --- ChannelLayout ---

pub type ChannelLayout = u8;

pub const CHANNEL_MONO: ChannelLayout = 1;
pub const CHANNEL_STEREO: ChannelLayout = 2;
pub const CHANNEL_SURROUND_2_1: ChannelLayout = 3;
pub const CHANNEL_QUAD: ChannelLayout = 4;
pub const CHANNEL_SURROUND_5_1: ChannelLayout = 6;
pub const CHANNEL_SURROUND_7_1: ChannelLayout = 8;

// --- ContentType ---

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentType(pub String);

impl ContentType {
  pub const MUSIC: &'static str = "music";
  pub const PODCAST: &'static str = "podcast";
  pub const AUDIOBOOK: &'static str = "audiobook";
  pub const RADIO: &'static str = "radio";
  pub const LIVE_STREAM: &'static str = "live_stream";
  pub const AMBIENT: &'static str = "ambient";

  pub fn new(value: impl Into<String>) -> Self {
    Self(value.into())
  }

  pub fn as_str(&self) -> &str {
    &self.0
  }
}

impl fmt::Display for ContentType {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(&self.0)
  }
}

impl From<&str> for ContentType {
  fn from(s: &str) -> Self {
    Self(s.to_string())
  }
}

impl From<String> for ContentType {
  fn from(s: String) -> Self {
    Self(s)
  }
}

/// Channel interleave mode for audio data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Interleave {
  /// Samples are interleaved: LRLRLR...
  Interleaved,
  /// Samples are planar: LLL...RRR...
  Planar,
}
// --- SampleSpec ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SampleSpec {
  pub sample_rate: SampleRate,
  pub channels: ChannelLayout,
  pub bit_depth: BitDepth,
  pub interleave: Interleave,
}

impl SampleSpec {
  pub fn new(
    sample_rate: SampleRate,
    channels: ChannelLayout,
    bit_depth: BitDepth,
    interleave: Interleave,
  ) -> Result<Self, &'static str> {
    if sample_rate == 0 {
      return Err("sample_rate must be > 0");
    }
    if channels == 0 || channels > 32 {
      return Err("channels must be in 1..=32");
    }
    Ok(Self {
      sample_rate,
      channels,
      bit_depth,
      interleave,
    })
  }

  /// Convenience: interleaved stereo at standard bit depth.
  pub fn interleaved(
    sample_rate: SampleRate,
    channels: ChannelLayout,
    bit_depth: BitDepth,
  ) -> Result<Self, &'static str> {
    Self::new(sample_rate, channels, bit_depth, Interleave::Interleaved)
  }
}

// --- ExtensionSource ---

pub type ExtensionSource = u8;

pub const SOURCE_BUILTIN: ExtensionSource = 0;
pub const SOURCE_STATIC: ExtensionSource = 1;
pub const SOURCE_DYNAMIC: ExtensionSource = 2;
pub const SOURCE_RUNTIME: ExtensionSource = 3;
pub const SOURCE_BYTES: ExtensionSource = 4;
pub const SOURCE_CONFIG: ExtensionSource = 5;
pub const SOURCE_DISCOVERED: ExtensionSource = 6;

// --- PlanarBuffer ---

/// Planar audio buffer with shared allocation.
///
/// Stores audio data in planar (non-interleaved) format: each channel
/// has its own contiguous region of `f32` samples within a single shared
/// `Vec<f32>` allocation. Channels are stored back-to-back.
///
/// This layout enables SIMD-friendly per-channel processing without
/// deinterleaving overhead.
pub struct PlanarBuffer {
  data: Vec<f32>,
  channels: usize,
  frames: usize,
}

impl PlanarBuffer {
  /// Create a new planar buffer with `channels` channels and `frames` frames per channel.
  ///
  /// # Panics
  ///
  /// Panics when `channels` is zero or `channels * frames` overflows `usize`.
  pub fn new(channels: usize, frames: usize) -> Self {
    assert!(channels > 0, "PlanarBuffer requires at least one channel");
    let Some(len) = channels.checked_mul(frames) else {
      panic!("PlanarBuffer dimensions overflow usize");
    };
    Self {
      data: vec![0.0f32; len],
      channels,
      frames,
    }
  }

  /// Fill from interleaved source data.
  ///
  /// `interleaved` must have exactly `channels * frames` elements,
  /// arranged as consecutive frames of interleaved samples.
  pub fn fill_from_interleaved(&mut self, interleaved: &[f32]) {
    assert_eq!(interleaved.len(), self.channels * self.frames);
    for (i, &sample) in interleaved.iter().enumerate() {
      let ch = i % self.channels;
      let frame = i / self.channels;
      self.data[ch * self.frames + frame] = sample;
    }
  }

  /// Write planar data into an interleaved output buffer.
  ///
  /// `interleaved` must have at least `channels * frames` elements.
  /// The output is written as consecutive frames of interleaved samples.
  pub fn write_interleaved(&self, interleaved: &mut [f32]) {
    assert!(interleaved.len() >= self.channels * self.frames);
    for ch in 0..self.channels {
      for frame in 0..self.frames {
        let src = ch * self.frames + frame;
        let dst = frame * self.channels + ch;
        interleaved[dst] = self.data[src];
      }
    }
  }

  /// Get a reference to a single channel's samples.
  ///
  /// # Panics
  ///
  /// Panics when `ch >= self.channels()`.
  pub fn channel(&self, ch: usize) -> &[f32] {
    assert!(ch < self.channels, "channel index out of bounds");
    let start = ch * self.frames;
    &self.data[start..start + self.frames]
  }

  /// Get a mutable reference to a single channel's samples.
  ///
  /// # Panics
  ///
  /// Panics when `ch >= self.channels()`.
  pub fn channel_mut(&mut self, ch: usize) -> &mut [f32] {
    assert!(ch < self.channels, "channel index out of bounds");
    let start = ch * self.frames;
    &mut self.data[start..start + self.frames]
  }

  /// Number of channels.
  pub fn channels(&self) -> usize {
    self.channels
  }

  /// Number of frames (samples per channel).
  pub fn frames(&self) -> usize {
    self.frames
  }

  /// Total length of the underlying data buffer (`channels * frames`).
  pub fn len(&self) -> usize {
    self.data.len()
  }

  /// Returns `true` if the buffer contains zero frames.
  pub fn is_empty(&self) -> bool {
    self.frames == 0
  }
}

#[cfg(test)]
mod tests {
  #![expect(
    clippy::unwrap_used,
    reason = "Test code: unwrap is acceptable on construction of test fixtures"
  )]
  use super::*;

  // --- SampleSpec validation ---

  #[test]
  fn interleaved_accepts_standard_params() {
    let spec = SampleSpec::interleaved(44100, 2, 16);
    assert!(spec.is_ok());
    let spec = spec.unwrap();
    assert_eq!(spec.sample_rate, 44100);
    assert_eq!(spec.channels, 2);
    assert_eq!(spec.bit_depth, 16);
    assert_eq!(spec.interleave, Interleave::Interleaved);
  }

  #[test]
  fn planar_accepts_standard_params() {
    let spec = SampleSpec::new(44100, 2, 24, Interleave::Planar);
    assert!(spec.is_ok());
    let spec = spec.unwrap();
    assert_eq!(spec.interleave, Interleave::Planar);
  }

  #[test]
  fn sample_spec_rejects_zero_sample_rate() {
    let err = SampleSpec::interleaved(0, 2, 16).unwrap_err();
    assert!(!err.is_empty());
  }

  #[test]
  fn sample_spec_rejects_zero_channels() {
    let err = SampleSpec::interleaved(44100, 0, 16).unwrap_err();
    assert!(!err.is_empty());
  }

  #[test]
  fn sample_spec_rejects_channels_above_max() {
    let err = SampleSpec::interleaved(44100, 33, 16).unwrap_err();
    assert!(!err.is_empty());
  }

  #[test]
  fn sample_spec_accepts_minimum_sample_rate() {
    let spec = SampleSpec::interleaved(1, 2, 16);
    assert!(spec.is_ok());
  }

  #[test]
  fn sample_spec_accepts_minimum_channels() {
    let spec = SampleSpec::interleaved(44100, 1, 16);
    assert!(spec.is_ok());
  }

  #[test]
  fn sample_spec_accepts_maximum_channels() {
    let spec = SampleSpec::interleaved(44100, 32, 16);
    assert!(spec.is_ok());
  }

  // --- PlanarBuffer layout and conversion ---

  #[test]
  fn planar_buffer_round_trips_interleaved_samples() {
    let input = [1.0, 10.0, 2.0, 20.0, 3.0, 30.0];
    let mut planar = PlanarBuffer::new(2, 3);

    planar.fill_from_interleaved(&input);

    assert_eq!(planar.channel(0), &[1.0, 2.0, 3.0]);
    assert_eq!(planar.channel(1), &[10.0, 20.0, 30.0]);
    let mut output = [0.0; 6];
    planar.write_interleaved(&mut output);
    assert_eq!(output, input);
  }

  #[test]
  fn planar_buffer_channel_mut_updates_only_selected_channel() {
    let mut planar = PlanarBuffer::new(2, 2);
    planar.channel_mut(1).copy_from_slice(&[0.25, 0.5]);

    assert_eq!(planar.channel(0), &[0.0, 0.0]);
    assert_eq!(planar.channel(1), &[0.25, 0.5]);
    assert_eq!(planar.channels(), 2);
    assert_eq!(planar.frames(), 2);
    assert_eq!(planar.len(), 4);
    assert!(!planar.is_empty());
  }

  #[test]
  fn planar_buffer_zero_frames_is_empty() {
    let planar = PlanarBuffer::new(2, 0);
    assert!(planar.is_empty());
    assert_eq!(planar.channel(0), &[]);
  }

  #[test]
  #[should_panic(expected = "at least one channel")]
  fn planar_buffer_rejects_zero_channels() {
    let _ = PlanarBuffer::new(0, 1);
  }
  // --- ContentType Display and From ---

  #[test]
  fn content_type_display_music() {
    let ct = ContentType::new(ContentType::MUSIC);
    assert_eq!(ct.to_string(), "music");
  }

  #[test]
  fn content_type_display_podcast() {
    let ct = ContentType::new(ContentType::PODCAST);
    assert_eq!(ct.to_string(), "podcast");
  }

  #[test]
  fn content_type_display_audiobook() {
    let ct = ContentType::new(ContentType::AUDIOBOOK);
    assert_eq!(ct.to_string(), "audiobook");
  }

  #[test]
  fn content_type_display_radio() {
    let ct = ContentType::new(ContentType::RADIO);
    assert_eq!(ct.to_string(), "radio");
  }

  #[test]
  fn content_type_display_live_stream() {
    let ct = ContentType::new(ContentType::LIVE_STREAM);
    assert_eq!(ct.to_string(), "live_stream");
  }

  #[test]
  fn content_type_display_ambient() {
    let ct = ContentType::new(ContentType::AMBIENT);
    assert_eq!(ct.to_string(), "ambient");
  }

  #[test]
  fn content_type_from_str() {
    let ct = ContentType::from("podcast");
    assert_eq!(ct.0, "podcast");
  }

  #[test]
  fn content_type_from_string() {
    let ct = ContentType::from(String::from("radio"));
    assert_eq!(ct.0, "radio");
  }

  #[test]
  fn content_type_as_str() {
    let ct = ContentType::new("music");
    assert_eq!(ct.as_str(), "music");
  }
}
