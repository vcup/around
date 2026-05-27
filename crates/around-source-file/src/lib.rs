//! around-source-file: Local filesystem Source implementation.

use around_core::{AroundError, Source, SourceCapabilities};
use std::fs;
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

pub struct FileSource {
  path: PathBuf,
  opened: AtomicBool,
}

impl FileSource {
  pub fn new(path: impl Into<PathBuf>) -> Self {
    Self {
      path: path.into(),
      opened: AtomicBool::new(false),
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

    self.opened.store(true, std::sync::atomic::Ordering::SeqCst);

    Ok(Box::new(BufReader::new(file)))
  }

  fn content_length(&self) -> Option<u64> {
    fs::metadata(&self.path).ok().map(|m| m.len())
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
