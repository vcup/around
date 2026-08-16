use around_core::{Source, SourceCapabilities};
use around_source_file::FileSource;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    .join("../../examples")
    .join(name)
}

#[test]
fn file_source_reports_correct_capabilities() {
  let src = FileSource::new(fixture_path("example.wav"));
  let caps = src.capabilities();
  assert!(caps.contains(SourceCapabilities::MULTI_OPEN));
  assert!(caps.contains(SourceCapabilities::SEEKABLE));
}

#[test]
#[expect(
  deprecated,
  clippy::expect_used,
  reason = "example.wav exists as a committed test fixture; open() and read() must succeed by contract for contract tests"
)]
fn file_source_open_returns_readable_stream() {
  let src = FileSource::new(fixture_path("example.wav"));
  let mut reader = src.open().expect("should open existing file");
  let mut buf = [0u8; 4];
  let n = reader.read(&mut buf).expect("should read");
  assert_eq!(n, 4);
  assert_eq!(&buf, b"RIFF");
}

#[expect(
  deprecated,
  reason = "Phase 1 backward compat: sync open()/open_seekable() retained for legacy tests"
)]
#[test]
fn file_source_open_nonexistent_file_returns_error() {
  let src = FileSource::new(fixture_path("nonexistent.wav"));
  let result = src.open();
  assert!(result.is_err());
}

#[test]
#[expect(
  clippy::expect_used,
  reason = "example.wav is a committed test fixture; content_length() returns the real file size for a known-good file"
)]
fn file_source_content_length_matches_file_size() {
  let src = FileSource::new(fixture_path("example.wav"));
  let len = src.content_length().expect("should know length");
  assert!(len > 0);
}

#[test]
#[expect(
  deprecated,
  clippy::unwrap_used,
  clippy::expect_used,
  reason = "zero.wav is a committed 0-byte test fixture; content_length(), open(), and read() are well-defined for empty files"
)]
fn file_source_empty_file() {
  let src = FileSource::new(fixture_path("zero.wav"));
  let len = src.content_length().expect("should know length");
  assert_eq!(len, 0);
  let mut reader = src.open().expect("should open zero-byte file");
  let mut buf = [0u8; 1];
  assert_eq!(reader.read(&mut buf).unwrap(), 0);
}

#[test]
#[expect(
  deprecated,
  clippy::expect_used,
  reason = "example.wav is a committed fixture with MULTI_OPEN capability; both opens succeed by contract"
)]
fn file_source_multi_open_allows_multiple_opens() {
  let src = FileSource::new(fixture_path("example.wav"));
  let _r1 = src.open().expect("first open");
  let _r2 = src.open().expect("second open — MULTI_OPEN");
}

#[test]
fn file_source_identifier_is_stable() {
  let src = FileSource::new(fixture_path("example.wav"));
  let id1 = src.identifier();
  let id2 = src.identifier();
  assert_eq!(id1, id2);
}

#[test]
#[expect(
  deprecated,
  clippy::expect_used,
  reason = "example.wav is a committed fixture with SEEKABLE capability; open_seekable(), read(), seek(), and read_exact() succeed by contract"
)]
fn file_source_open_seekable_returns_seekable_stream() {
  let src = FileSource::new(fixture_path("example.wav"));
  let mut reader = src.open_seekable().expect("should open seekable");
  let mut buf = [0u8; 4];
  let n = reader.read(&mut buf).expect("should read");
  assert_eq!(n, 4);
  assert_eq!(&buf, b"RIFF");
  // Verify seek works
  reader
    .seek(SeekFrom::Start(0))
    .expect("should seek to start");
  let mut buf2 = [0u8; 4];
  reader.read_exact(&mut buf2).expect("should re-read");
  assert_eq!(&buf2, b"RIFF");
}

#[test]
#[expect(
  deprecated,
  clippy::expect_used,
  reason = "example.wav is a committed fixture; open() and open_seekable() both produce valid independent streams for MULTI_OPEN sources"
)]
fn file_source_open_seekable_is_independent_from_open() {
  // MULTI_OPEN: open() and open_seekable() create independent streams.
  let src = FileSource::new(fixture_path("example.wav"));
  let mut r1 = src.open().expect("open");
  let mut r2 = src.open_seekable().expect("open_seekable");
  let mut buf = [0u8; 4];
  r1.read_exact(&mut buf).expect("read from open");
  assert_eq!(&buf, b"RIFF");
  r2.read_exact(&mut buf).expect("read from open_seekable");
  assert_eq!(&buf, b"RIFF");
}

// ── Async Source tests (Phase 2) ──────────────────────────────────────────

#[tokio::test]
#[expect(
  clippy::expect_used,
  reason = "example.wav is a committed test fixture; async read() must succeed by contract"
)]
async fn file_source_async_read_returns_data() {
  let mut src = FileSource::new(fixture_path("example.wav"));
  let mut buf = [0u8; 1024];
  let n = src.read(&mut buf).await.expect("async read should succeed");
  assert!(n > 0, "should read at least some bytes");
  // First 4 bytes should be RIFF header
  assert_eq!(&buf[..4], b"RIFF");
}

#[tokio::test]
#[expect(
  clippy::expect_used,
  reason = "example.wav is a committed fixture with SEEKABLE capability; async seek() must succeed"
)]
async fn file_source_async_seek_and_reread() {
  let mut src = FileSource::new(fixture_path("example.wav"));
  let mut buf = [0u8; 4];
  let n = src.read(&mut buf).await.expect("first read");
  assert_eq!(n, 4);
  assert_eq!(&buf, b"RIFF");

  // Seek back to start
  let pos = src
    .seek(SeekFrom::Start(0))
    .await
    .expect("seek should succeed");
  assert_eq!(pos, 0);

  // Re-read — should be RIFF again
  let mut buf2 = [0u8; 4];
  let n2 = src.read(&mut buf2).await.expect("re-read after seek");
  assert_eq!(n2, 4);
  assert_eq!(&buf2, b"RIFF");
}

#[tokio::test]
#[expect(
  clippy::expect_used,
  reason = "zero.wav is a committed 0-byte fixture; async read should return 0 at EOF"
)]
async fn file_source_async_read_empty_file_returns_zero() {
  let mut src = FileSource::new(fixture_path("zero.wav"));
  let mut buf = [0u8; 16];
  let n = src.read(&mut buf).await.expect("async read on empty file");
  assert_eq!(n, 0, "empty file should return 0 bytes");
}
