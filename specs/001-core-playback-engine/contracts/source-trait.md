# Contract: Source Trait

**Version**: 0.1.0-pre
**Stability**: Pre-1.0 (may evolve; see constitution API Stability Path — parity with Decoder trait stability policy).

## Trait Definition

```rust
use bitflags::bitflags;

bitflags! {
    pub struct SourceCapabilities: u8 {
        const NONE        = 0;
        const MULTI_OPEN  = 1 << 0;  // open() may be called multiple times
        const SEEKABLE    = 1 << 1;  // underlying storage supports random access
    }
}

/// Helper trait combining Read + Seek for use in trait objects.
pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

pub trait Source: Send + Sync {
    /// Capabilities this source provides. The engine consults this to
    /// determine what operations are valid for this source instance.
    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities::NONE
    }

    /// Open the source and return a readable byte stream.
    /// If MULTI_OPEN is NOT set, successive calls may return
    /// Err(AroundError::SourceAlreadyConsumed).
    fn open(&self) -> Result<Box<dyn Read + Send>, AroundError>;

    /// Open the source and return a seekable byte stream.
    /// Only valid when SEEKABLE capability is set.
    /// Default implementation returns Err for non-seekable sources.
    fn open_seekable(&self) -> Result<Box<dyn ReadSeek + Send + Sync>, AroundError> {
        Err(AroundError::Internal {
            message: "source does not support seeking".into(),
        })
    }

    /// Total content length in bytes, if known.
    fn content_length(&self) -> Option<u64>;

    /// MIME type hint, if available.
    fn content_type(&self) -> Option<String>;

    /// Human-readable identifier (file path, URL).
    fn identifier(&self) -> String;
}
```

## Contract

1. **Open-or-Fail**: `open()` MUST return a readable byte stream or an error. `open_seekable()` MUST return a seekable byte stream or an error. Sources that set `MULTI_OPEN` guarantee subsequent calls to both `open()` and `open_seekable()` also succeed; sources without it MAY fail with `SourceAlreadyConsumed` on a second call.

2. **Length Honesty**: `content_length()` MUST return `Some(n)` only when the total byte count is definitively known. Guessing is prohibited. Local files return `Some`; network streams return `None` unless Content-Length is present.

3. **Seekability via Capabilities**: `capabilities()` MUST include `SEEKABLE` only when the underlying storage supports random access. Sources that set `SEEKABLE` SHOULD implement `open_seekable()` to return a useful seekable stream, rather than relying on the default error implementation.

4. **No I/O in Metadata Methods**: `content_type()`, `content_length()`, `capabilities()`, and `identifier()` MUST NOT perform I/O. I/O happens only in `open()` and `open_seekable()`.

5. **Identifier Stability**: `identifier()` MUST return a stable, human-readable string for the same logical source across calls. It is used for error messages and logging, not for equality comparisons or persistent keys.

6. **Thread Safety**: `Send + Sync`. Multiple threads MAY call metadata methods concurrently.

## Test Fixtures

Contract tests require:
- An existing file on disk (valid source, sets MULTI_OPEN | SEEKABLE)
- A path to a non-existent file (error case)
- A zero-byte file (empty source edge case)
- A single-shot source (MULTI_OPEN not set) to verify second-open rejection
- A non-seekable source to verify `open_seekable()` returns the default error
