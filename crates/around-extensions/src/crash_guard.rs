use std::panic::{catch_unwind, AssertUnwindSafe, UnwindSafe};

/// Error returned when an extension operation panics.
#[derive(Debug, Clone)]
pub struct CrashError {
  /// The panic payload, extracted for diagnostics.
  pub payload: Option<String>,
  /// Which guarded operation was running.
  pub operation: &'static str,
  /// Which extension (name from ExtensionMeta) caused the panic, if known.
  pub extension: Option<String>,
}

impl std::fmt::Display for CrashError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    if let Some(ref ext) = self.extension {
      write!(
        f,
        "{} panicked in extension '{}': {}",
        self.operation,
        ext,
        self.payload.as_deref().unwrap_or("<no payload>")
      )
    } else {
      write!(
        f,
        "{} panicked: {}",
        self.operation,
        self.payload.as_deref().unwrap_or("<no payload>")
      )
    }
  }
}

impl std::error::Error for CrashError {}

/// Wrapper around `catch_unwind` that logs panic information.
///
/// All extension entry points should be wrapped in `crash_guard` or
/// `crash_guard_unsafe`.  The guard records the crash and returns an
/// error instead of propagating the panic.
pub fn crash_guard<F, R>(
  operation: &'static str,
  extension: Option<String>,
  f: F,
) -> Result<R, CrashError>
where
  F: FnOnce() -> R + UnwindSafe,
{
  match catch_unwind(f) {
    Ok(result) => Ok(result),
    Err(payload) => {
      let msg = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| format!("{:?}", payload));

      tracing::error!(
          operation,
          extension = extension.as_deref().unwrap_or("<unknown>"),
          panic = %msg,
          "extension crash guarded"
      );

      Err(CrashError {
        payload: Some(msg),
        operation,
        extension,
      })
    }
  }
}

/// Convenience wrapper that uses `AssertUnwindSafe`.
///
/// This is the common case since most extension callbacks accept raw
/// pointers and therefore are not `UnwindSafe`.
pub fn crash_guard_unsafe<F, R>(
  operation: &'static str,
  extension: Option<String>,
  f: F,
) -> Result<R, CrashError>
where
  F: FnOnce() -> R,
{
  crash_guard(operation, extension, AssertUnwindSafe(f))
}
