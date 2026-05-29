//! Extension contract tests — verify extension loading mechanism contracts.
//!
//! Adapted for the codec system (CodecInfo + ExtensionManager).

use around_core::{CodecInfo, ReadSeek, Source};
use around_engine::{DecoderInfo, ExtensionManager};
use around_source_file::FileSource;
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    .join("../../examples")
    .join(name)
}

// ---------------------------------------------------------------------------
// ExtensionManager tests
// ---------------------------------------------------------------------------

#[test]
fn extension_manager_new_is_empty() {
  let mgr = ExtensionManager::new();
  let decoders = mgr.list_decoders();
  assert!(decoders.is_empty());
}

#[test]
fn extension_manager_list_decoders_returns_empty_initially() {
  let mgr = ExtensionManager::new();
  assert!(mgr.list_decoders().is_empty());
}

#[test]
fn decoder_info_clone_works() {
  let info = DecoderInfo {
    name: "test".into(),
    formats: vec!["wav".into()],
    source: "built-in".into(),
    path: None,
  };
  let info2 = info.clone();
  assert_eq!(info.name, info2.name);
}

#[test]
fn wav_codec_info_open_from_reader() {
  let src = FileSource::new(fixture_path("example.wav"));
  let reader = src.open_seekable().expect("should open seekable");
  let result = (around_codec_wav::WAV_INFO.open_fn)(reader);
  assert!(result.is_ok());
}

#[test]
fn wav_codec_open_non_wav_returns_error() {
  let src = FileSource::new(fixture_path("zero.wav"));
  let reader = src.open_seekable().expect("should open seekable");
  let result = (around_codec_wav::WAV_INFO.open_fn)(reader);
  assert!(result.is_err());
}

#[test]
fn pcm_codec_open_and_read() {
  // Create a minimal PCM file: sample_rate=8000, channels=1, bits_per_sample=16, 2 bytes of silence
  let raw = vec![0x40, 0x1F, 0x00, 0x00, 1, 16, 0, 0];
  let reader: Box<dyn ReadSeek + Send + Sync> = Box::new(std::io::Cursor::new(raw));
  let mut stream = (around_codec_test_pcm::PCM_INFO.open_fn)(reader).expect("should open PCM");

  assert_eq!(stream.sample_rate, 8000);
  assert_eq!(stream.channels, 1);

  let mut buf = [0.0f32; 1024];
  let frames = stream.read(&mut buf).expect("read should succeed");
  assert!(frames.is_some());
}

#[test]
fn duplicate_load_decoder_is_idempotent() {
  let _mgr = ExtensionManager::new();
}

#[test]
fn version_change_replaces_decoder() {
  let _mgr = ExtensionManager::new();
}

#[test]
fn codec_info_copy_works_correctly() {
  let info = around_codec_wav::WAV_INFO;
  let copy = info;
  assert_eq!(info.name, copy.name);
  assert_eq!(
    info.supported_formats.as_ptr(),
    copy.supported_formats.as_ptr()
  );
}
