//! Source trait and related types.

use crate::error::AroundError;
use bitflags::bitflags;
use std::io::{Read, Seek};

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SourceCapabilities: u8 {
        const NONE = 0;
        /// open() may be called multiple times
        const MULTI_OPEN = 1 << 0;
        /// Underlying storage supports random access
        const SEEKABLE = 1 << 1;
    }
}

/// Helper trait combining `Read` + `Seek` for use in trait objects.
/// Rust only allows one non-auto trait in a `dyn` type; this merges them.
pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

/// Trait abstracting where audio bytes come from.
///
/// Implementations include local files, HTTP streams, and in-memory buffers.
/// All I/O happens in `open()` and `open_seekable()`; metadata methods must not
/// perform I/O.
pub trait Source: Send + Sync {
  /// Capabilities this source provides.
  fn capabilities(&self) -> SourceCapabilities {
    SourceCapabilities::NONE
  }

  /// Open the source and return a readable byte stream.
  /// If MULTI_OPEN is NOT set, successive calls may return
  /// Err(AroundError::SourceAlreadyConsumed).
  fn open(&self) -> Result<Box<dyn Read + Send>, AroundError>;

  /// Open the source and return a seekable byte stream.
  /// Only valid when SEEKABLE capability is set.
  /// Default: returns Err for non-seekable sources.
  fn open_seekable(&self) -> Result<Box<dyn ReadSeek + Send + Sync>, AroundError> {
    Err(AroundError::Internal {
      message: "source does not support seeking".into(),
    })
  }

  /// Total content length in bytes, if known.
  fn content_length(&self) -> Option<u64>;

  /// MIME type hint, if available.
  fn content_type(&self) -> Option<String>;

  /// Human-readable identifier (file path, URL, etc.).
  fn identifier(&self) -> String;
}

/// Describes a format signature a decoder can match against.
#[derive(Debug, Clone)]
pub struct FormatSignature {
  pub extension: Option<&'static str>,
  pub mime_type: Option<&'static str>,
  pub magic_bytes: Option<&'static [u8]>,
  pub description: &'static str,
}

impl FormatSignature {
  pub const fn from_extension(extension: &'static str, description: &'static str) -> Self {
    Self {
      extension: Some(extension),
      mime_type: None,
      magic_bytes: None,
      description,
    }
  }

  pub const fn with_mime(self, mime_type: &'static str) -> Self {
    Self {
      mime_type: Some(mime_type),
      ..self
    }
  }

  pub const fn with_magic(self, magic_bytes: &'static [u8]) -> Self {
    Self {
      magic_bytes: Some(magic_bytes),
      ..self
    }
  }
}
