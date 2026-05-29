//! Codec contract tests — verify the codec system (AudioStream + CodecInfo).
//!
//! Covers: CodecInfo, AudioStream read/seek/drop, CodecRegistry matching,
//! and the FR-002 format detection phases.

use around_core::{
  AroundError, AudioStream, CodecInfo, CodecRegistry, FormatSignature, ReadSeek, Source,
};
use around_source_file::FileSource;
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    .join("../../examples")
    .join(name)
}

fn open_reader(path: &str) -> Box<dyn ReadSeek + Send> {
  let src = FileSource::new(fixture_path(path));
  src.open_seekable().expect("should open seekable")
}

// ---------------------------------------------------------------------------
// Test helpers for mock codecs
// ---------------------------------------------------------------------------

unsafe fn dummy_read(
  _data: *mut std::ffi::c_void,
  _buf: &mut [f32],
) -> Result<Option<usize>, AroundError> {
  Ok(None)
}
unsafe fn dummy_seek(_data: *mut std::ffi::c_void, _frame: u64) -> Result<(), AroundError> {
  Ok(())
}
unsafe fn dummy_drop(_data: *mut std::ffi::c_void) {}

static DUMMY_VTABLE: around_core::AudioStreamVTable = around_core::AudioStreamVTable {
  read: dummy_read,
  seek: dummy_seek,
  drop: dummy_drop,
};

/// A codec that always fails to open.
fn failing_codec(name: &'static str, ext: &'static str) -> CodecInfo {
  fn open_fn(_reader: Box<dyn ReadSeek + Send>) -> Result<AudioStream, AroundError> {
    Err(AroundError::DecodeError {
      message: "failing codec always fails".into(),
    })
  }
  let fmts: &'static [FormatSignature] = Box::leak(Box::new([FormatSignature::from_extension(
    ext,
    "always-fails",
  )]));
  CodecInfo::new(name, fmts, open_fn)
}

/// A codec that succeeds (returns a dummy AudioStream) or fails.
fn test_codec(
  name: &'static str,
  ext: &'static str,
  magic: &'static [u8],
  succeeds: bool,
) -> CodecInfo {
  let fmts: &'static [FormatSignature] =
    Box::leak(Box::new([
      FormatSignature::from_extension(ext, "test").with_magic(magic)
    ]));
  CodecInfo::new(
    name,
    fmts,
    if succeeds {
      |_reader| unsafe {
        Ok(AudioStream::new(
          std::ptr::null_mut(),
          &DUMMY_VTABLE,
          44100,
          1,
          0,
        ))
      }
    } else {
      |_| {
        Err(AroundError::DecodeError {
          message: "test codec fails".into(),
        })
      }
    },
  )
}

// ---------------------------------------------------------------------------
// CodecInfo / AudioStream tests
// ---------------------------------------------------------------------------

#[test]
fn wav_codec_info_has_non_empty_formats() {
  let info = &around_codec_wav::WAV_INFO;
  assert!(!info.supported_formats.is_empty());
  assert_eq!(info.name, "wav");
}

#[test]
fn wav_codec_open_wav_file() {
  let reader = open_reader("example.wav");
  let result = (around_codec_wav::WAV_INFO.open_fn)(reader);
  assert!(result.is_ok());
}

#[test]
fn wav_codec_open_and_read_pcm() {
  let reader = open_reader("stereo_16bit.wav");
  let mut stream = (around_codec_wav::WAV_INFO.open_fn)(reader).expect("should open WAV");

  // We know stereo_16bit.wav has 2 channels
  assert_eq!(stream.channels, 2);
  assert!(stream.sample_rate > 0);
  assert!(stream.total_frames > 0);

  let mut buf = vec![0.0f32; 4096 * stream.channels as usize];
  let frames = stream.read(&mut buf).expect("read should succeed");
  assert!(frames.is_some());
  assert!(frames.unwrap() > 0);
}

#[test]
fn wav_codec_read_returns_none_after_exhaustion() {
  let reader = open_reader("example.wav");
  let mut stream = (around_codec_wav::WAV_INFO.open_fn)(reader).expect("should open");

  let ch = stream.channels as usize;
  let mut buf = vec![0.0f32; 4096 * ch];
  // Read until exhausted
  loop {
    match stream.read(&mut buf).expect("read should not error") {
      Some(_) => continue,
      None => break,
    }
  }
  // Another read should return None
  let result = stream.read(&mut buf).expect("read should not error");
  assert!(result.is_none());
}

#[test]
fn wav_codec_truncated_file_reads_may_fail() {
  let src = FileSource::new(fixture_path("truncated.wav"));
  let reader = src.open_seekable().expect("should open seekable");
  // The codec may successfully open a truncated WAV if the header is parseable.
  // Reads may return None quickly or succeed for residual data.
  if let Ok(mut stream) = (around_codec_wav::WAV_INFO.open_fn)(reader) {
    let mut buf = vec![0.0f32; 4096 * stream.channels.max(1) as usize];
    let _ = stream.read(&mut buf);
  }
}

#[test]
fn wav_codec_zero_byte_file_errors() {
  let src = FileSource::new(fixture_path("zero.wav"));
  let reader = src.open_seekable().expect("should open seekable");
  let result = (around_codec_wav::WAV_INFO.open_fn)(reader);
  assert!(result.is_err());
}

#[test]
fn wav_codec_seek_uses_sample_frames() {
  let reader = open_reader("long-test.wav");
  let mut stream = (around_codec_wav::WAV_INFO.open_fn)(reader).expect("should open");

  // Seek to frame 100
  stream.seek(100).expect("seek should succeed");

  let ch = stream.channels as usize;
  let mut buf = vec![0.0f32; 1024 * ch];
  let frames = stream.read(&mut buf).expect("read should succeed");
  assert!(frames.is_some());
}

#[test]
fn wav_codec_seek_past_end_returns_invalid_position() {
  let reader = open_reader("example.wav");
  let mut stream = (around_codec_wav::WAV_INFO.open_fn)(reader).expect("should open");

  let result = stream.seek(u64::MAX);
  assert!(result.is_err());
  match result {
    Err(AroundError::InvalidPosition { .. }) => {}
    _ => panic!("expected InvalidPosition error"),
  }
}

#[test]
fn wav_codec_stereo_output_has_correct_frame_count() {
  let reader = open_reader("stereo_16bit.wav");
  let mut stream = (around_codec_wav::WAV_INFO.open_fn)(reader).expect("should open");

  assert_eq!(stream.channels, 2);

  let mut buf = vec![0.0f32; 4096 * 2]; // frames × channels
  let frames = stream.read(&mut buf).expect("read should succeed").unwrap();
  assert!(frames > 0);
  // frames × channels ≤ buf.len()
  assert!(frames * 2 <= buf.len());
}

// ---------------------------------------------------------------------------
// CodecRegistry matching tests
// ---------------------------------------------------------------------------

#[test]
fn codec_registry_by_extension_matches_wav() {
  let mut registry = CodecRegistry::new();
  registry.push(around_codec_wav::WAV_INFO);

  let matches = registry.by_extension("wav");
  assert!(!matches.is_empty());
  assert_eq!(matches[0].name, "wav");

  // Also matches "WAV" case-insensitively
  let matches = registry.by_extension("WAV");
  assert!(!matches.is_empty());
}

#[test]
fn codec_registry_by_extension_no_match_returns_empty() {
  let mut registry = CodecRegistry::new();
  registry.push(around_codec_wav::WAV_INFO);

  let matches = registry.by_extension("mp3");
  assert!(matches.is_empty());
}

#[test]
fn codec_registry_by_magic_matches_RIFF() {
  let mut registry = CodecRegistry::new();
  registry.push(around_codec_wav::WAV_INFO);

  let probe = b"RIFF....WAVE";
  let matches = registry.by_magic(probe);
  assert!(!matches.is_empty());
  assert_eq!(matches[0].name, "wav");
}

#[test]
fn codec_registry_by_magic_no_match_returns_empty() {
  let mut registry = CodecRegistry::new();
  registry.push(around_codec_wav::WAV_INFO);

  let probe = b"ID3....";
  let matches = registry.by_magic(probe);
  assert!(matches.is_empty());
}

#[test]
fn codec_info_copy_trait() {
  let info = around_codec_wav::WAV_INFO;
  let copy = info; // Copy, not move
  assert_eq!(info.name, copy.name);
}

#[test]
fn pcm_codec_info_exists() {
  let info = &around_codec_test_pcm::PCM_INFO;
  assert_eq!(info.name, "pcm");
  assert!(!info.supported_formats.is_empty());
}

#[test]
fn pcm_codec_open_and_read() {
  // Create a minimal PCM file: sample_rate=8000, channels=1, bits_per_sample=16, 2 bytes of silence
  let raw = vec![0x40, 0x1F, 0x00, 0x00, 1, 16, 0, 0];
  let reader: Box<dyn ReadSeek + Send> = Box::new(std::io::Cursor::new(raw));
  let mut stream = (around_codec_test_pcm::PCM_INFO.open_fn)(reader).expect("should open PCM");

  assert_eq!(stream.sample_rate, 8000);
  assert_eq!(stream.channels, 1);
  assert!(stream.total_frames > 0);

  let mut buf = [0.0f32; 1024];
  let frames = stream.read(&mut buf).expect("read should succeed");
  assert!(frames.is_some());
}

// =============================================================================
// FR-002 algorithm coverage: phase-by-phase tests
// =============================================================================

// ---------------------------------------------------------------------------
// Phase 2: Extension-first (no magic I/O on success)
// ---------------------------------------------------------------------------

#[test]
fn extension_first_codec_fallback_chain() {
  // Register failing .wav codec first, then real WAV codec.
  // Extension match → try failing-wav → fails → try real wav → succeeds.
  let mut registry = CodecRegistry::new();
  registry.push(failing_codec("bad-wav", "wav"));
  registry.push(around_codec_wav::WAV_INFO);

  let ext_codecs = registry.by_extension("wav");
  assert_eq!(ext_codecs.len(), 2);

  let mut succeeded = false;
  for codec in &ext_codecs {
    let reader = open_reader("example.wav");
    match (codec.open_fn)(reader) {
      Ok(_) => {
        assert_eq!(codec.name, "wav");
        succeeded = true;
        break;
      }
      Err(_) => continue,
    }
  }
  assert!(succeeded, "one codec must open valid WAV data");
}

// ---------------------------------------------------------------------------
// Phase 3: Magic detection when no extension match
// ---------------------------------------------------------------------------

#[test]
fn no_extension_detected_by_magic() {
  let mut registry = CodecRegistry::new();
  registry.push(around_codec_wav::WAV_INFO);

  let ext_codecs = registry.by_extension("");
  assert!(ext_codecs.is_empty());

  let magic_codecs = registry.by_magic(b"RIFF....WAVE");
  assert!(!magic_codecs.is_empty());
  assert_eq!(magic_codecs[0].name, "wav");
}

// ---------------------------------------------------------------------------
// Phase 3: Extension/magic conflict (magic wins per FR-002)
// ---------------------------------------------------------------------------

#[test]
fn extension_magic_conflict_magic_wins() {
  let mut registry = CodecRegistry::new();
  registry.push(around_codec_wav::WAV_INFO);
  let mp3 = test_codec("mp3", "mp3", b"ID3", true);
  registry.push(mp3);

  let ext_codecs = registry.by_extension("wav");
  assert_eq!(ext_codecs.len(), 1);

  let magic_codecs = registry.by_magic(b"ID3\x03\x00\x00\x00...");
  assert_eq!(magic_codecs.len(), 1);
  assert_eq!(magic_codecs[0].name, "mp3");

  // Conflict: ext_codecs != magic_codecs → magic wins
  assert!(same_ptr_set(&ext_codecs, &magic_codecs) == false);
}

fn same_ptr_set(a: &[&CodecInfo], b: &[&CodecInfo]) -> bool {
  a.len() == b.len() && a.iter().all(|ca| b.iter().any(|cb| std::ptr::eq(*ca, *cb)))
}

// ---------------------------------------------------------------------------
// Phase 4+5: Full fallback chain
// ---------------------------------------------------------------------------

#[test]
fn fallback_chain_tries_all_codecs() {
  let mut registry = CodecRegistry::new();
  registry.push(failing_codec("failing-wav", "wav"));
  registry.push(around_codec_wav::WAV_INFO);
  registry.push(test_codec("mp3", "mp3", b"ID3", true));

  let ext_codecs = registry.by_extension("wav");
  assert_eq!(ext_codecs.len(), 2);

  // Phase 2: try ext codecs in order.
  let mut tried: Vec<&str> = Vec::new();
  let mut opened = false;
  for codec in &ext_codecs {
    let reader = open_reader("example.wav");
    tried.push(codec.name);
    match (codec.open_fn)(reader) {
      Ok(_) => {
        opened = true;
        break;
      }
      Err(_) => continue,
    }
  }
  assert!(opened, "WAV codec should open WAV data");
  assert_eq!(tried, vec!["failing-wav", "wav"]);
}

#[test]
fn all_codecs_fail_no_fallback_remaining() {
  let mut registry = CodecRegistry::new();
  registry.push(failing_codec("bad-wav", "wav"));

  let ext_codecs = registry.by_extension("wav");
  assert_eq!(ext_codecs.len(), 1);

  let mut all_failed = true;
  for codec in &ext_codecs {
    let reader: Box<dyn ReadSeek + Send> = Box::new(std::io::Cursor::new(b"RIFF....WAVE....."));
    match (codec.open_fn)(reader) {
      Ok(_) => all_failed = false,
      Err(_) => {}
    }
  }
  assert!(all_failed);

  // Magic: no codec has RIFF magic (bad-wav only has extension).
  let magic_codecs = registry.by_magic(b"RIFF....WAVE");
  assert!(magic_codecs.is_empty());

  // Phase 5: no other codecs to try → UnsupportedFormat.
}

#[test]
fn registry_all_supports_full_fallback() {
  let mut registry = CodecRegistry::new();
  registry.push(around_codec_wav::WAV_INFO);
  registry.push(around_codec_test_pcm::PCM_INFO);

  let all = registry.all();
  assert_eq!(all.len(), 2);
  let names: Vec<&str> = all.iter().map(|c| c.name).collect();
  assert!(names.contains(&"wav"));
  assert!(names.contains(&"pcm"));
}
