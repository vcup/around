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

/// Trait for audio format decoders.
///
/// Implementations decode specific audio formats (WAV, MP3, FLAC, etc.)
/// and produce interleaved f32 PCM samples in [-1.0, 1.0].
///
/// All implementations must be Send + Sync. read() and seek() are single-threaded;
/// metadata() and output_format() may be called concurrently.
pub trait Decoder: Send + Sync {
  /// Returns the MIME types and format signatures this decoder handles.
  fn supported_formats() -> &'static [FormatSignature];

  /// Declares what capabilities the Source must provide for this decoder.
  /// The engine MUST verify the source satisfies these before calling open().
  fn source_requirements() -> SourceRequirements;

  /// Quick check: can this decoder handle the given source?
  /// MUST NOT consume bytes from the source.
  fn can_decode(source: &dyn Source) -> bool;

  /// Open the source and create a decoding stream.
  fn open(source: Box<dyn Source>) -> Result<Self, AroundError>
  where
    Self: Sized;

  /// Read the next chunk of decoded PCM samples as interleaved f32 in [-1.0, 1.0].
  /// Decoder is responsible for converting native sample format to f32.
  /// Returns Ok(None) when the stream is exhausted.
  fn read(&mut self, buf: &mut [f32]) -> Result<Option<usize>, AroundError>;

  /// Seek to a byte offset within the stream (if supported).
  fn seek(&mut self, offset: u64) -> Result<(), AroundError>;

  /// Extract metadata from the source.
  fn metadata(&self) -> &Metadata;

  /// Return the output format (sample rate, channels).
  fn output_format(&self) -> SampleSpec;
}
