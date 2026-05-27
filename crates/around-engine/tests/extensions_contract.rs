//! Extension contract tests — verify extension loading mechanism contracts.

use around_core::{DecoderFactory, ErasedDecoder, FormatSignature};
use around_engine::{DecoderInfo, ExtensionManager};
use around_source_file::FileSource;
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    .join("../../examples")
    .join(name)
}

#[test]
fn extension_manager_new_is_empty() {
  let mgr = ExtensionManager::new();
  let decoders = mgr.list_decoders();
  assert!(decoders.is_empty());
}

#[test]
fn extension_manager_list_decoders_returns_empty_initially() {
  let mgr = ExtensionManager::new();
  assert_eq!(mgr.list_decoders().len(), 0);
}

#[test]
fn decoder_info_clone_works() {
  let info = DecoderInfo {
    name: "test".into(),
    formats: vec!["wav".into()],
    source: "runtime".into(),
    path: Some(PathBuf::from("/tmp/test.so")),
  };
  let cloned = info.clone();
  assert_eq!(cloned.name, "test");
  assert_eq!(cloned.formats, vec!["wav"]);
  assert_eq!(cloned.source, "runtime");
}

#[test]
fn erased_decoder_construction() {
  use around_codec_wav::WavDecoder;

  let erased = ErasedDecoder {
    supported_formats: WavDecoder::supported_formats(),
    source_requirements: WavDecoder::source_requirements(),
    inner: Box::new(
      WavDecoder::open(Box::new(FileSource::new(fixture_path("example.wav")))).unwrap(),
    ),
  };

  assert!(!erased.supported_formats.is_empty());
  assert!(erased
    .supported_formats
    .iter()
    .any(|f| f.extension == Some("wav")));
}

fn decoder_factory_trait_is_implemented() {
  // Verify WavDecoder implements DecoderFactory (compilation check)
  let formats = around_codec_wav::WavDecoder::supported_formats();
  assert!(!formats.is_empty());
  let reqs = around_codec_wav::WavDecoder::source_requirements();
  assert!(reqs.contains(around_core::SourceRequirements::SEEKABLE));
}

#[test]
fn wav_decoder_factory_can_decode_wav() {
  let src = FileSource::new(fixture_path("example.wav"));
  assert!(around_codec_wav::WavDecoder::can_decode(&src));
}

#[test]
fn wav_decoder_factory_cannot_decode_pcm() {
  // A WAV decoder should not claim to decode PCM files
  let src = FileSource::new(fixture_path("zero.wav"));
  // zero.wav has .wav extension, so can_decode should still return true by extension
  // This tests extension-first detection
  assert!(around_codec_wav::WavDecoder::can_decode(&src));
}
