//! Codec system: universal types for audio format detection and decoding.
//!
//! # Architecture
//!
//! - [`AudioStream`] — runtime handle with fn-pointer dispatch (zero `dyn Trait`)
//! - [`CodecInfo`] — static identity: name, format claims, constructor
//! - [`CodecRegistry`] — ordered collection of [`CodecInfo`] for detection/selection
//! - [`PrefixReader`] — wraps non-seekable `Read` into `ReadSeek` via internal buffer
//!
//! Hot path: `stream.read(buf)` → `(self.vtable.read)(self.data, buf)` — single
//! indirect call, no vtable lookup. Complies with Constitution §III.
//!
//! Engine tracks seek capability from `source.capabilities()`, not from
//! `AudioStream`. Engine tracks codec name from the selection loop, not from
//! `AudioStream`. `AudioStream` is a pure runtime interface.

use crate::error::AroundError;
use crate::source::{FormatSignature, ReadSeek};
use std::io::{self, Read, Seek, SeekFrom};

// ---------------------------------------------------------------------------
// AudioStream — runtime handle
// ---------------------------------------------------------------------------

/// Runtime handle for an opened audio codec.
///
/// Fields `sample_rate`, `channels`, `total_frames` are populated by the codec
/// during `open()`. The engine reads them directly (zero-cost field access).
/// `total_frames` is 0 when the stream length is unknown.
pub struct AudioStream {
  data: *mut std::ffi::c_void,
  vtable: &'static AudioStreamVTable,
  pub sample_rate: u32,
  pub channels: u8,
  pub total_frames: u64,
}

/// SAFETY: `AudioStream` is `Send` because the codec owns its data exclusively.
/// The vtable is a `&'static` reference (immortal, immutable).
unsafe impl Send for AudioStream {}
/// SAFETY: `AudioStream` is `Sync` because `read()` and `seek()` take `&mut self`,
/// which guarantees exclusive access — the caller must not share concurrently.
unsafe impl Sync for AudioStream {}

/// Private vtable — never exposed outside this module.
/// Private vtable — constructed once per codec in a `static`, never exposed
/// outside the codec crate. The engine only interacts through `AudioStream`.
pub struct AudioStreamVTable {
  pub read: unsafe fn(*mut std::ffi::c_void, buf: &mut [f32]) -> Result<Option<usize>, AroundError>,
  pub seek: unsafe fn(*mut std::ffi::c_void, frame: u64) -> Result<(), AroundError>,
  pub drop: unsafe fn(*mut std::ffi::c_void),
}

impl AudioStream {
  /// Create a new stream from opaque codec data, a vtable, and format metadata.
  ///
  /// # Safety
  ///
  /// Caller ensures `data` is a valid pointer to heap-allocated codec state
  /// that can be freed by `vtable.drop`, and that `vtable.read`/`vtable.seek`
  /// expect `data` in the correct concrete type.
  pub unsafe fn new(
    data: *mut std::ffi::c_void,
    vtable: &'static AudioStreamVTable,
    sample_rate: u32,
    channels: u8,
    total_frames: u64,
  ) -> Self {
    Self {
      data,
      vtable,
      sample_rate,
      channels,
      total_frames,
    }
  }

  /// Read the next chunk of decoded PCM frames as interleaved f32.
  /// Returns the number of **frames** decoded, or `None` at end-of-stream.
  #[inline]
  pub fn read(&mut self, buf: &mut [f32]) -> Result<Option<usize>, AroundError> {
    unsafe { (self.vtable.read)(self.data, buf) }
  }

  /// Seek to a frame offset within the decoded PCM stream.
  #[inline]
  pub fn seek(&mut self, frame: u64) -> Result<(), AroundError> {
    unsafe { (self.vtable.seek)(self.data, frame) }
  }
}

impl Drop for AudioStream {
  #[inline]
  fn drop(&mut self) {
    if !self.data.is_null() {
      unsafe { (self.vtable.drop)(self.data) };
    }
    self.data = std::ptr::null_mut();
  }
}

// ---------------------------------------------------------------------------
// CodecInfo — static codec identity
// ---------------------------------------------------------------------------

/// Static descriptor for a codec: name, format claims, and an `open` constructor.
///
/// Codec crates export a `pub static INFO: CodecInfo`. The engine pushes it into
/// [`CodecRegistry`] explicitly (no `linkme`, no magic).
#[derive(Clone, Copy)]
pub struct CodecInfo {
  pub name: &'static str,
  pub supported_formats: &'static [FormatSignature],
  /// Open a byte stream positioned at the start of audio data.
  /// Returns `Err` if the format does not match or parsing fails.
  pub open_fn: fn(Box<dyn ReadSeek + Send>) -> Result<AudioStream, AroundError>,
}

impl CodecInfo {
  /// Construct a new [`CodecInfo`].
  pub const fn new(
    name: &'static str,
    supported_formats: &'static [FormatSignature],
    open_fn: fn(Box<dyn ReadSeek + Send>) -> Result<AudioStream, AroundError>,
  ) -> Self {
    Self {
      name,
      supported_formats,
      open_fn,
    }
  }
}

impl std::fmt::Debug for CodecInfo {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("CodecInfo")
      .field("name", &self.name)
      .field("formats", &self.supported_formats)
      .finish_non_exhaustive()
  }
}

// ---------------------------------------------------------------------------
// CodecRegistry — ordered collection
// ---------------------------------------------------------------------------

/// Ordered collection of [`CodecInfo`] entries.
///
/// The engine registers built-in codecs at startup and dynamic codecs at
/// load time. Order matters: entries registered first are tried first in
/// the FR-002 fallback chain.
#[derive(Default)]
pub struct CodecRegistry {
  entries: Vec<CodecInfo>,
}

impl CodecRegistry {
  pub fn new() -> Self {
    Self {
      entries: Vec::new(),
    }
  }

  /// Push a codec into the registry. Earlier entries have higher priority.
  pub fn push(&mut self, info: CodecInfo) {
    self.entries.push(info);
  }

  /// Return all registered codecs in registration order.
  pub fn all(&self) -> &[CodecInfo] {
    &self.entries
  }

  /// Codecs whose [`FormatSignature::extension`] matches `ext` (case-insensitive).
  pub fn by_extension(&self, ext: &str) -> Vec<&CodecInfo> {
    let ext_lower = ext.to_lowercase();
    self
      .entries
      .iter()
      .filter(|c| {
        c.supported_formats.iter().any(|f| {
          f.extension
            .is_some_and(|e| e.eq_ignore_ascii_case(&ext_lower))
        })
      })
      .collect()
  }

  /// Codecs whose [`FormatSignature::magic_bytes`] is a prefix of `probe`.
  pub fn by_magic(&self, probe: &[u8]) -> Vec<&CodecInfo> {
    self
      .entries
      .iter()
      .filter(|c| {
        c.supported_formats.iter().any(|f| {
          f.magic_bytes
            .is_some_and(|magic| probe.len() >= magic.len() && probe.starts_with(magic))
        })
      })
      .collect()
  }
}

// ---------------------------------------------------------------------------
// PrefixReader — non-seekable → seekable adapter
// ---------------------------------------------------------------------------

/// Wraps a non-seekable `Read` stream and implements `Read + Seek` by caching
/// all bytes read so far in an internal buffer.
///
/// Seeks within the buffer succeed; seeks beyond it fail. `rewind()` resets the
/// logical position without discarding cached bytes — this enables multiple
/// codec attempts without re-reading from the underlying source.
pub struct PrefixReader<R: Read> {
  inner: R,
  /// Ring-buffer of bytes read so far. `len` grows monotonically.
  buffer: Vec<u8>,
  /// Current logical read position (0..buffer.len()).
  pos: usize,
  /// Capacity hint for the initial buffer allocation.
  capacity: usize,
}

impl<R: Read> PrefixReader<R> {
  /// Create a new [`PrefixReader`] wrapping `inner` with an initial buffer
  /// capacity of `capacity` bytes.
  pub fn new(inner: R, capacity: usize) -> Self {
    Self {
      inner,
      buffer: Vec::with_capacity(capacity),
      pos: 0,
      capacity,
    }
  }

  /// Reset the logical position to 0 without discarding buffered data.
  /// This allows the next reader to start from the beginning.
  pub fn rewind(&mut self) {
    self.pos = 0;
  }

  /// Return a reference to the buffered data.
  pub fn buffer(&self) -> &[u8] {
    &self.buffer
  }

  /// Ensure `need` bytes are available in the buffer by reading from the
  /// underlying stream. Returns the number of buffered bytes available.
  fn fill_to(&mut self, need: usize) -> io::Result<usize> {
    while self.buffer.len() < need {
      let mut chunk = vec![0u8; self.capacity];
      let n = self.inner.read(&mut chunk)?;
      if n == 0 {
        break;
      }
      self.buffer.extend_from_slice(&chunk[..n]);
    }
    Ok(self.buffer.len())
  }
}

impl<R: Read> Read for PrefixReader<R> {
  fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
    // Ensure we have enough buffered data.
    let need = self.pos + buf.len();
    let available = self.fill_to(need)?;
    if self.pos >= available {
      return Ok(0);
    }
    let to_copy = std::cmp::min(buf.len(), available - self.pos);
    buf[..to_copy].copy_from_slice(&self.buffer[self.pos..self.pos + to_copy]);
    self.pos += to_copy;
    Ok(to_copy)
  }
}

impl<R: Read> Seek for PrefixReader<R> {
  fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
    let target = match pos {
      SeekFrom::Start(offset) => offset as i64,
      SeekFrom::End(_) => {
        return Err(io::Error::new(
          io::ErrorKind::Unsupported,
          "PrefixReader does not support SeekFrom::End",
        ))
      }
      SeekFrom::Current(offset) => self.pos as i64 + offset,
    };

    if target < 0 {
      return Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "negative seek position",
      ));
    }

    let target = target as usize;

    // If seeking forward beyond what we've buffered, try to fill.
    if target > self.buffer.len() {
      let available = self.fill_to(target)?;
      if target > available {
        return Err(io::Error::new(
          io::ErrorKind::UnexpectedEof,
          format!(
            "seek to {} beyond buffered data ({} bytes available)",
            target, available
          ),
        ));
      }
    }

    self.pos = target;
    Ok(self.pos as u64)
  }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  #[test]
  fn prefix_reader_read_and_rewind() {
    let data = b"hello world, this is a test";
    let inner = Cursor::new(data.as_slice());
    let mut pr = PrefixReader::new(inner, 8);

    let mut buf = [0u8; 5];
    let n = pr.read(&mut buf).unwrap();
    assert_eq!(n, 5);
    assert_eq!(&buf, b"hello");

    pr.rewind();

    let mut buf2 = [0u8; 11];
    let n2 = pr.read(&mut buf2).unwrap();
    assert_eq!(n2, 11);
    assert_eq!(&buf2, b"hello world");
  }

  #[test]
  fn prefix_reader_seek_within_buffer() {
    let data = b"0123456789abcdef";
    let inner = Cursor::new(data.as_slice());
    let mut pr = PrefixReader::new(inner, 16);

    // Read first 4 bytes to fill buffer.
    let mut buf = [0u8; 4];
    pr.read(&mut buf).unwrap();
    assert_eq!(&buf, b"0123");

    // Seek back to offset 2.
    pr.seek(SeekFrom::Start(2)).unwrap();
    let mut buf2 = [0u8; 4];
    pr.read(&mut buf2).unwrap();
    assert_eq!(&buf2, b"2345");
  }

  #[test]
  fn prefix_reader_seek_beyond_buffer_fails() {
    let data = b"abc";
    let inner = Cursor::new(data.as_slice());
    let mut pr = PrefixReader::new(inner, 16);

    let result = pr.seek(SeekFrom::Start(100));
    assert!(result.is_err());
  }

  #[test]
  fn codec_registry_by_extension() {
    static FMTS: &[FormatSignature] = &[FormatSignature::from_extension("wav", "WAV")];
    let info = CodecInfo::new("test", FMTS, |_| unreachable!());
    let mut reg = CodecRegistry::new();
    reg.push(info);

    let matches = reg.by_extension("WAV");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].name, "test");

    let matches = reg.by_extension("mp3");
    assert!(matches.is_empty());
  }

  #[test]
  fn codec_registry_by_magic() {
    static FMTS: &[FormatSignature] =
      &[FormatSignature::from_extension("wav", "WAV").with_magic(b"RIFF")];
    let info = CodecInfo::new("riff", FMTS, |_| unreachable!());
    let mut reg = CodecRegistry::new();
    reg.push(info);

    let probe = b"RIFF....WAVE";
    let matches = reg.by_magic(probe);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].name, "riff");

    let probe = b"ID3....";
    let matches = reg.by_magic(probe);
    assert!(matches.is_empty());
  }
}
