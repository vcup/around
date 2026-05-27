//! around-core: Shared types, traits, and error types for the around audio engine.
//!
//! This crate defines the foundational contracts (`Decoder`, `Source`)
//! that all other crates build upon. It has minimal dependencies.

pub mod decoder;
pub mod error;
pub mod metadata;
pub mod source;
pub mod state;
pub mod types;

// Re-export all public types at crate root for convenience
pub use decoder::{Decoder, SourceRequirements};
pub use error::AroundError;
pub use metadata::Metadata;
pub use source::{FormatSignature, Source, SourceCapabilities};
pub use state::{PlaybackStatus, PAUSED, PLAYING, STOPPED};
pub use types::{
  AudioFormat, BitDepth, ChannelLayout, ContentType, ExtensionSource, SampleRate, SampleSpec,
  BIT_DEPTH_16, BIT_DEPTH_24, BIT_DEPTH_32, BIT_DEPTH_64, BIT_DEPTH_8, CHANNEL_MONO, CHANNEL_QUAD,
  CHANNEL_STEREO, CHANNEL_SURROUND_2_1, CHANNEL_SURROUND_5_1, CHANNEL_SURROUND_7_1,
  SAMPLE_RATE_176400, SAMPLE_RATE_192000, SAMPLE_RATE_44100, SAMPLE_RATE_48000, SAMPLE_RATE_88200,
  SAMPLE_RATE_96000, SOURCE_BUILTIN, SOURCE_BYTES, SOURCE_CONFIG, SOURCE_DISCOVERED,
  SOURCE_DYNAMIC, SOURCE_RUNTIME, SOURCE_STATIC,
};
