//! Source trait and related types.

use crate::error::AroundError;
use bitflags::bitflags;
use std::io::{Read, Seek, SeekFrom};

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

/// Type alias for async read/seek return types.
/// A pinned, heap-allocated future that resolves to `Result<T, AroundError>`.
pub type SourceFuture<'a, T> =
  std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, AroundError>> + Send + 'a>>;

/// Trait abstracting where audio bytes come from.
///
/// Implementations include local files, HTTP streams, and in-memory buffers.
/// The primary I/O interface is through the async [`read`] and [`seek`] methods.
/// Metadata methods (`content_length`, `content_type`, `identifier`) must not perform I/O.
///
/// # Async compatibility
///
/// The trait uses `Pin<Box<dyn Future<…>>>` return types for async methods to remain
/// object-safe (compatible with `dyn Source`). This is equivalent to what `#[async_trait]`
/// generates, without the proc-macro dependency.
///
/// # Backward compatibility
///
/// The deprecated [`open`] and [`open_seekable`] methods provide a sync compatibility
/// path. New sources should implement [`read`] and [`seek`] instead. The default
/// implementations of [`open`] and [`open_seekable`] return an error.
pub trait Source: Send + Sync {
  /// Capabilities this source provides.
  fn capabilities(&self) -> SourceCapabilities {
    SourceCapabilities::NONE
  }

  /// Read bytes from the source into `buf`, returning the number of bytes read.
  ///
  /// Returns `Ok(0)` to indicate end-of-file. This is the primary I/O method;
  /// all async sources implement this instead of [`open`].
  fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> SourceFuture<'a, usize>;

  /// Seek to a byte position within the source, if supported.
  ///
  /// Returns the new position from the start of the stream.
  /// The default implementation returns an "unsupported" error.
  fn seek<'a>(&'a mut self, _pos: SeekFrom) -> SourceFuture<'a, u64> {
    Box::pin(async move {
      Err(AroundError::Internal {
        message: "source does not support seeking".into(),
      })
    })
  }

  /// Total content length in bytes, if known.
  fn content_length(&self) -> Option<u64>;

  /// MIME type hint, if available.
  fn content_type(&self) -> Option<String>;

  /// Human-readable identifier (file path, URL, etc.).
  fn identifier(&self) -> String;

  // ── Deprecated sync compatibility methods ──────────────────────────────
  // These exist so that the current synchronous codec probe / decode loop
  // in `pipeline.rs` continues to compile.  New Source implementations
  // should NOT override them — they are provided with a default error return.

  /// Open the source and return a readable byte stream.
  ///
  /// NOTE: Deprecated. New code should use [`read`] directly.
  /// Default returns an error indicating the source only supports async access.
  #[deprecated(
    since = "0.1.0",
    note = "Use async read() instead. This method is retained only for Phase 1 backward compatibility."
  )]
  fn open(&self) -> Result<Box<dyn Read + Send>, AroundError> {
    Err(AroundError::Internal {
      message: "source only supports async read()".into(),
    })
  }

  /// Open the source and return a seekable byte stream.
  ///
  /// NOTE: Deprecated. New code should use [`read`] and [`seek`] directly.
  /// Default returns an error indicating the source only supports async access.
  #[deprecated(
    since = "0.1.0",
    note = "Use async read()/seek() instead. This method is retained only for Phase 1 backward compatibility."
  )]
  fn open_seekable(&self) -> Result<Box<dyn ReadSeek + Send + Sync>, AroundError> {
    Err(AroundError::Internal {
      message: "source only supports async read()/seek()".into(),
    })
  }
}

/// Describes a format signature a codec can match against.
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn source_capabilities_none_contains_nothing() {
    let caps = SourceCapabilities::NONE;
    assert!(!caps.contains(SourceCapabilities::MULTI_OPEN));
    assert!(!caps.contains(SourceCapabilities::SEEKABLE));
  }

  #[test]
  fn source_capabilities_contains_individual_flag() {
    let caps = SourceCapabilities::MULTI_OPEN;
    assert!(caps.contains(SourceCapabilities::MULTI_OPEN));
    assert!(!caps.contains(SourceCapabilities::SEEKABLE));
  }

  #[test]
  fn source_capabilities_or_combines_flags() {
    let caps = SourceCapabilities::MULTI_OPEN | SourceCapabilities::SEEKABLE;
    assert!(caps.contains(SourceCapabilities::MULTI_OPEN));
    assert!(caps.contains(SourceCapabilities::SEEKABLE));
  }

  #[test]
  fn source_capabilities_empty_bits() {
    let caps = SourceCapabilities::NONE;
    assert_eq!(caps.bits(), 0);
  }

  #[test]
  fn source_capabilities_seekable_bits() {
    let caps = SourceCapabilities::SEEKABLE;
    assert_eq!(caps.bits(), 1 << 1);
  }
}
