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

// --- SampleSpec ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleSpec {
  pub sample_rate: SampleRate,
  pub channels: ChannelLayout,
  pub bit_depth: BitDepth,
}

impl SampleSpec {
  pub fn new(
    sample_rate: SampleRate,
    channels: ChannelLayout,
    bit_depth: BitDepth,
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
    })
  }
}

// --- AudioFormat ---

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFormat {
  pub container: String,
  pub codec: String,
  pub mime_type: String,
  pub sample_spec: SampleSpec,
  pub bitrate: Option<u64>,
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
