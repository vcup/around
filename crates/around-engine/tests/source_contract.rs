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
fn file_source_open_returns_readable_stream() {
  let src = FileSource::new(fixture_path("example.wav"));
  let mut reader = src.open().expect("should open existing file");
  let mut buf = [0u8; 4];
  let n = reader.read(&mut buf).expect("should read");
  assert_eq!(n, 4);
  assert_eq!(&buf, b"RIFF");
}

#[test]
fn file_source_open_nonexistent_file_returns_error() {
  let src = FileSource::new(fixture_path("nonexistent.wav"));
  let result = src.open();
  assert!(result.is_err());
}

#[test]
fn file_source_content_length_matches_file_size() {
  let src = FileSource::new(fixture_path("example.wav"));
  let len = src.content_length().expect("should know length");
  assert!(len > 0);
}

#[test]
fn file_source_empty_file() {
  let src = FileSource::new(fixture_path("zero.wav"));
  let len = src.content_length().expect("should know length");
  assert_eq!(len, 0);
  let mut reader = src.open().expect("should open zero-byte file");
  let mut buf = [0u8; 1];
  assert_eq!(reader.read(&mut buf).unwrap(), 0);
}

#[test]
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
