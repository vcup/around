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

impl From<std::io::Error> for AroundError {
  fn from(e: std::io::Error) -> Self {
    match e.kind() {
      std::io::ErrorKind::NotFound => Self::FileNotFound {
        path: e.to_string(),
      },
      _ => Self::Internal {
        message: format!("I/O error: {}", e),
      },
    }
  }
}

impl From<String> for AroundError {
  fn from(s: String) -> Self {
    Self::Internal { message: s }
  }
}

impl From<&str> for AroundError {
  fn from(s: &str) -> Self {
    Self::Internal {
      message: s.to_string(),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn error_display_file_not_found() {
    let err = AroundError::FileNotFound {
      path: "/tmp/x.wav".into(),
    };
    let msg = err.to_string();
    assert!(!msg.is_empty());
    assert!(msg.contains("file not found"));
    assert!(msg.contains("/tmp/x.wav"));
  }

  #[test]
  fn error_display_unsupported_format() {
    let err = AroundError::UnsupportedFormat {
      format: Some("flac".into()),
      reason: "no codec available".into(),
    };
    let msg = err.to_string();
    assert!(!msg.is_empty());
    assert!(msg.contains("unsupported format"));
  }

  #[test]
  fn error_display_decode_error() {
    let err = AroundError::DecodeError {
      message: "corrupt frame".into(),
    };
    let msg = err.to_string();
    assert!(!msg.is_empty());
    assert!(msg.contains("decode error"));
  }

  #[test]
  fn error_display_source_incompatible() {
    let err = AroundError::SourceIncompatible {
      source: "http://example.com/stream".into(),
      required: "seekable".into(),
    };
    let msg = err.to_string();
    assert!(!msg.is_empty());
    assert!(msg.contains("incompatible"));
  }

  #[test]
  fn error_display_no_track() {
    let err = AroundError::NoTrack;
    let msg = err.to_string();
    assert!(!msg.is_empty());
    assert!(msg.contains("no track"));
  }

  #[test]
  fn error_display_codec_load_failed() {
    let err = AroundError::CodecLoadFailed {
      path: Some("libfoo.so".into()),
      reason: "symbol not found".into(),
    };
    let msg = err.to_string();
    assert!(!msg.is_empty());
    assert!(msg.contains("codec load failed"));
  }

  #[test]
  fn error_display_internal() {
    let err = AroundError::Internal {
      message: "unexpected null".into(),
    };
    let msg = err.to_string();
    assert!(!msg.is_empty());
    assert!(msg.contains("internal error"));
  }

  #[test]
  fn error_display_source_already_consumed() {
    let err = AroundError::SourceAlreadyConsumed {
      source: "file".into(),
    };
    let msg = err.to_string();
    assert!(!msg.is_empty());
    assert!(msg.contains("already consumed"));
  }

  #[test]
  fn error_display_invalid_position() {
    let err = AroundError::InvalidPosition {
      position_ms: 5000,
      duration_ms: Some(3000),
    };
    let msg = err.to_string();
    assert!(!msg.is_empty());
    assert!(msg.contains("invalid position"));
  }

  #[test]
  fn error_display_codec_not_supported() {
    let err = AroundError::CodecNotSupported {
      codec: "aac".into(),
      reason: "license required".into(),
    };
    let msg = err.to_string();
    assert!(!msg.is_empty());
    assert!(msg.contains("not supported"));
  }

  #[test]
  fn error_display_unsupported_format_no_name() {
    let err = AroundError::UnsupportedFormat {
      format: None,
      reason: "unknown container".into(),
    };
    let msg = err.to_string();
    assert!(!msg.is_empty());
    assert!(msg.contains("unsupported format"));
  }

  #[test]
  fn error_display_codec_load_failed_no_path() {
    let err = AroundError::CodecLoadFailed {
      path: None,
      reason: "no suitable decoder".into(),
    };
    let msg = err.to_string();
    assert!(!msg.is_empty());
    assert!(msg.contains("codec load failed"));
  }

  #[test]
  fn error_display_invalid_position_no_duration() {
    let err = AroundError::InvalidPosition {
      position_ms: 9999,
      duration_ms: None,
    };
    let msg = err.to_string();
    assert!(!msg.is_empty());
    assert!(msg.contains("invalid position"));
  }
}
