//! Decoder trait and related types.

use crate::error::AroundError;
use crate::metadata::Metadata;
use crate::source::{FormatSignature, Source};
use crate::types::SampleSpec;
use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SourceRequirements: u8 {
        const NONE = 0;
        const SEEKABLE = 1 << 0;
        const KNOWN_LENGTH = 1 << 1;
        const KNOWN_CONTENT_TYPE = 1 << 2;
    }
}

/// Runtime interface for an opened audio decoder — object-safe.
///
/// `read()` and `seek()` are single-threaded; `metadata()` and `output_format()`
/// may be called concurrently.
pub trait Decoder: Send + Sync {
  /// Read the next chunk of decoded PCM samples as interleaved f32 in [-1.0, 1.0].
  /// Returns Ok(None) when the stream is exhausted.
  fn read(&mut self, buf: &mut [f32]) -> Result<Option<usize>, AroundError>;

  /// Seek to a sample offset within the stream (if supported).
  fn seek(&mut self, offset: u64) -> Result<(), AroundError>;

  /// Extract metadata from the source.
  fn metadata(&self) -> &Metadata;

  /// Return the output format (sample rate, channels, bit depth).
  fn output_format(&self) -> SampleSpec;
}

/// Static decoder metadata and constructor — not object-safe by design.
///
/// This trait is implemented per decoder type and used at setup time
/// (format detection, capability checks, decoder instantiation).
pub trait DecoderFactory: Decoder + Sized {
  /// Returns the MIME types and format signatures this decoder handles.
  fn supported_formats() -> &'static [FormatSignature];

  /// Declares what capabilities the Source must provide for this decoder.
  fn source_requirements() -> SourceRequirements;

  /// Quick check: can this decoder handle the given source?
  fn can_decode(source: &dyn Source) -> bool;

  /// Open the source and create a decoding stream.
  fn open(source: Box<dyn Source>) -> Result<Self, AroundError>;
}

/// Type-erased box around a decoder that also exposes factory info.
///
/// Used by the extension manager to hold loaded decoders and query
/// their format support without knowing the concrete type.
pub struct ErasedDecoder {
  pub inner: Box<dyn Decoder>,
  pub supported_formats: &'static [FormatSignature],
  pub source_requirements: SourceRequirements,
}
