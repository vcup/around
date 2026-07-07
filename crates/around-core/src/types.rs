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

#[cfg(test)]
mod tests {
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
