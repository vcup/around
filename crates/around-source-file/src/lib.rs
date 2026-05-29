//! around-source-file: Local filesystem Source implementation.

use around_core::{AroundError, Source, SourceCapabilities};
use std::fs;
use std::io::{BufReader, Read};
use std::path::PathBuf;

pub struct FileSource {
  path: PathBuf,
  /// Snapshot of file size at construction time. MAY diverge if the file is
  /// externally modified after construction; the engine uses this as a hint.
  content_length: Option<u64>,
}

impl FileSource {
  pub fn new(path: impl Into<PathBuf>) -> Self {
    let path = path.into();
    let content_length = fs::metadata(&path).ok().map(|m| m.len());
    Self {
      path,
      content_length,
    }
  }
}

impl Source for FileSource {
  fn capabilities(&self) -> SourceCapabilities {
    SourceCapabilities::MULTI_OPEN | SourceCapabilities::SEEKABLE
  }

  fn open(&self) -> Result<Box<dyn Read + Send>, AroundError> {
    let file = fs::File::open(&self.path).map_err(|e| {
      if e.kind() == std::io::ErrorKind::NotFound {
        AroundError::FileNotFound {
          path: self.path.display().to_string(),
        }
      } else {
        AroundError::Internal {
          message: format!("failed to open '{}': {}", self.path.display(), e),
        }
      }
    })?;

    Ok(Box::new(BufReader::new(file)))
  }

  fn open_seekable(
    &self,
  ) -> Result<Box<dyn around_core::source::ReadSeek + Send + Sync>, AroundError> {
    let file = fs::File::open(&self.path).map_err(|e| {
      if e.kind() == std::io::ErrorKind::NotFound {
        AroundError::FileNotFound {
          path: self.path.display().to_string(),
        }
      } else {
        AroundError::Internal {
          message: format!("failed to open '{}': {}", self.path.display(), e),
        }
      }
    })?;

    Ok(Box::new(BufReader::new(file)))
  }

  fn content_length(&self) -> Option<u64> {
    self.content_length
  }

  fn content_type(&self) -> Option<String> {
    self
      .path
      .extension()
      .and_then(|ext| ext.to_str())
      .map(|ext| format!("audio/{}", ext.to_lowercase()))
  }

  fn identifier(&self) -> String {
    self.path.display().to_string()
  }
}
