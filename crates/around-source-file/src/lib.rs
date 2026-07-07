//! around-source-file: Local filesystem Source implementation.
//!
//! Provides both the async [`read`](around_core::Source::read) / [`seek`](around_core::Source::seek)
//! interface (via `tokio::fs::File`) and a sync compatibility layer
//! ([`open`](around_core::Source::open) / [`open_seekable`](around_core::Source::open_seekable))
//! for the current synchronous decode loop.

use around_core::{AroundError, Source, SourceCapabilities};
use std::future::Future;
use std::io::SeekFrom;
use std::path::PathBuf;
use std::pin::Pin;

pub struct FileSource {
  file: Option<tokio::fs::File>,
  path: PathBuf,
  /// Snapshot of file size at construction time. MAY diverge if the file is
  /// externally modified after construction; the engine uses this as a hint.
  content_length: Option<u64>,
}

impl FileSource {
  pub fn new(path: impl Into<PathBuf>) -> Self {
    let path = path.into();
    // Metadata lookup is sync here because it runs during Source construction,
    // before any async context is available.  This is acceptable per the
    // async pipeline design — metadata is not data I/O.
    let content_length = std::fs::metadata(&path).ok().map(|m| m.len());
    Self {
      file: None,
      path,
      content_length,
    }
  }
}

impl Source for FileSource {
  fn capabilities(&self) -> SourceCapabilities {
    SourceCapabilities::MULTI_OPEN | SourceCapabilities::SEEKABLE
  }

  fn read<'a>(
    &'a mut self,
    buf: &'a mut [u8],
  ) -> Pin<Box<dyn Future<Output = Result<usize, AroundError>> + Send + 'a>> {
    Box::pin(async move {
      use tokio::io::AsyncReadExt;

      // Lazy-open on first read — defer file I/O until an async context
      // is available.
      if self.file.is_none() {
        let file = tokio::fs::File::open(&self.path).await.map_err(|e| {
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
        self.file = Some(file);
      }

      let file = self.file.as_mut().expect("file was just opened — handle is Some");
      let n = file.read(buf).await.map_err(|e| AroundError::Internal {
        message: format!("read error '{}': {}", self.path.display(), e),
      })?;
      Ok(n)
    })
  }

  fn seek<'a>(
    &'a mut self,
    pos: SeekFrom,
  ) -> Pin<Box<dyn Future<Output = Result<u64, AroundError>> + Send + 'a>> {
    Box::pin(async move {
      use tokio::io::AsyncSeekExt;

      let file = self.file.as_mut().ok_or_else(|| AroundError::Internal {
        message: format!("seek before read on '{}'", self.path.display()),
      })?;

      file.seek(pos).await.map_err(|e| AroundError::Internal {
        message: format!("seek error '{}': {}", self.path.display(), e),
      })
    })
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

  // ── Sync compatibility layer ──────────────────────────────────────────
  // FileSource can provide sync open()/open_seekable() because it works
  // with local files — opening a std::fs::File is fast and doesn't require
  // an async runtime.

  fn open(&self) -> Result<Box<dyn std::io::Read + Send>, AroundError> {
    let file = std::fs::File::open(&self.path).map_err(|e| {
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
    Ok(Box::new(std::io::BufReader::new(file)))
  }

  fn open_seekable(
    &self,
  ) -> Result<Box<dyn around_core::source::ReadSeek + Send + Sync>, AroundError> {
    let file = std::fs::File::open(&self.path).map_err(|e| {
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
    Ok(Box::new(std::io::BufReader::new(file)))
  }
}
