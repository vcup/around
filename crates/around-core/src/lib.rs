//! around-core: Shared types, traits, and error types for the around audio engine.
//!
//! This crate defines the foundational contracts (`Source`) that all other
//! crates build upon. It has minimal dependencies.
//!
//! NOTE: Codec types (Codec trait, DynCodecRef, SafeCodecRef, etc.) moved to
//! around-audio-sdk (see ADR-0004).  Import from `around_audio_sdk::codec`
//! instead of `around_core::codec`.
pub mod audio_sink;
pub mod error;
pub mod metadata;
pub mod source;
pub mod state;
pub mod types;

pub use error::AroundError;

pub use audio_sink::{AudioSink, NullSink};
pub use metadata::Metadata;
pub use source::{ReadSeek, Source, SourceCapabilities};
pub use state::PlaybackStatus;
pub use types::{
  BitDepth, ChannelLayout, ContentType, ExtensionSource, Interleave, PlanarBuffer, SampleRate,
  SampleSpec, BIT_DEPTH_16, BIT_DEPTH_24, BIT_DEPTH_32, BIT_DEPTH_64, BIT_DEPTH_8, CHANNEL_MONO,
  CHANNEL_QUAD, CHANNEL_STEREO, CHANNEL_SURROUND_2_1, CHANNEL_SURROUND_5_1, CHANNEL_SURROUND_7_1,
  SAMPLE_RATE_176400, SAMPLE_RATE_192000, SAMPLE_RATE_44100, SAMPLE_RATE_48000, SAMPLE_RATE_88200,
  SAMPLE_RATE_96000, SOURCE_BUILTIN, SOURCE_BYTES, SOURCE_CONFIG, SOURCE_DISCOVERED,
  SOURCE_DYNAMIC, SOURCE_RUNTIME, SOURCE_STATIC,
};
