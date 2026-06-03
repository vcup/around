//! Error types for the around audio engine.

use std::fmt;

#[derive(Debug)]
pub enum AroundError {
  FileNotFound {
    path: String,
  },
  UnsupportedFormat {
    format: Option<String>,
    reason: String,
  },
  DecodeError {
    message: String,
  },
  SourceIncompatible {
    source: String,
    required: String,
  },
  NoTrack,
  CodecLoadFailed {
    path: Option<String>,
    reason: String,
  },
  Internal {
    message: String,
  },
  SourceAlreadyConsumed {
    source: String,
  },
  InvalidPosition {
    position_ms: u64,
    duration_ms: Option<u64>,
  },
  CodecNotSupported {
    codec: String,
    reason: String,
  },
}

impl fmt::Display for AroundError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::FileNotFound { path } => write!(f, "file not found: {}", path),
      Self::UnsupportedFormat { format, reason } => {
        if let Some(fmt) = format {
          write!(f, "unsupported format '{}': {}", fmt, reason)
        } else {
          write!(f, "unsupported format: {}", reason)
        }
      }
      Self::DecodeError { message } => write!(f, "decode error: {}", message),
      Self::SourceIncompatible { source, required } => {
        write!(
          f,
          "source '{}' is incompatible: requires {}",
          source, required
        )
      }
      Self::NoTrack => write!(f, "no track loaded"),
      Self::CodecLoadFailed { path, reason } => {
        if let Some(p) = path {
          write!(f, "codec load failed for '{}': {}", p, reason)
        } else {
          write!(f, "codec load failed: {}", reason)
        }
      }
      Self::Internal { message } => write!(f, "internal error: {}", message),
      Self::SourceAlreadyConsumed { source } => {
        write!(f, "source '{}' already consumed", source)
      }
      Self::InvalidPosition {
        position_ms,
        duration_ms,
      } => {
        if let Some(dur) = duration_ms {
          write!(
            f,
            "invalid position {}ms (duration: {}ms)",
            position_ms, dur
          )
        } else {
          write!(
            f,
            "invalid position {}ms (no duration available)",
            position_ms
          )
        }
      }
      Self::CodecNotSupported { codec, reason } => {
        write!(f, "codec '{}' not supported: {}", codec, reason)
      }
    }
  }
}

impl std::error::Error for AroundError {}
