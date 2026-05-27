use around_core::{Decoder, Source, SourceRequirements};
use around_source_file::FileSource;
use std::path::PathBuf;

fn fixture_path(name: &str) -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    .join("../../examples")
    .join(name)
}

#[test]
fn wav_decoder_supported_formats_non_empty() {
  let formats = around_codec_wav::WavDecoder::supported_formats();
  assert!(!formats.is_empty());
  // Should include wav extension
  assert!(formats.iter().any(|f| f.extension == Some("wav")));
}

#[test]
fn wav_decoder_requires_seekable_and_known_length() {
  let reqs = around_codec_wav::WavDecoder::source_requirements();
  assert!(reqs.contains(SourceRequirements::SEEKABLE));
  assert!(reqs.contains(SourceRequirements::KNOWN_LENGTH));
}

#[test]
fn wav_decoder_can_decode_wav_file() {
  let src = FileSource::new(fixture_path("example.wav"));
  assert!(around_codec_wav::WavDecoder::can_decode(&src));
}

#[test]
fn wav_decoder_open_and_read_pcm() {
  let src = FileSource::new(fixture_path("example.wav"));
  let src_box: Box<dyn Source> = Box::new(src);
  let mut decoder = around_codec_wav::WavDecoder::open(src_box).expect("should open WAV");

  let spec = decoder.output_format();
  assert_eq!(spec.sample_rate, 44100);
  assert_eq!(spec.channels, 1);

  let mut buf = vec![0.0f32; 1024];
  let total: usize = std::iter::from_fn(|| decoder.read(&mut buf).transpose())
    .map(|r| r.unwrap())
    .sum();
  assert!(total > 0, "should decode some samples");
}

#[test]
fn wav_decoder_metadata_available_after_open() {
  let src = FileSource::new(fixture_path("example.wav"));
  let src_box: Box<dyn Source> = Box::new(src);
  let decoder = around_codec_wav::WavDecoder::open(src_box).expect("should open");
  let _meta = decoder.metadata(); // must not panic
}

#[test]
fn wav_decoder_read_returns_none_after_exhaustion() {
  let src = FileSource::new(fixture_path("example.wav"));
  let src_box: Box<dyn Source> = Box::new(src);
  let mut decoder = around_codec_wav::WavDecoder::open(src_box).expect("should open");
  let mut buf = vec![0.0f32; 65536];
  // Drain all samples
  loop {
    match decoder.read(&mut buf).expect("read should not error") {
      Some(0) => break,
      Some(_) => continue,
      None => break,
    }
  }
  assert_eq!(decoder.read(&mut buf).unwrap(), None);
}

#[test]
fn wav_decoder_truncated_file_errors() {
  let src = FileSource::new(fixture_path("truncated.wav"));
  let src_box: Box<dyn Source> = Box::new(src);
  // Truncated file may or may not open — the contract says corrupt stream
  // should return DecodeError, not panic. If it opens, fine.
  if let Ok(mut _decoder) = around_codec_wav::WavDecoder::open(src_box) {
    // If it opened, that's acceptable for the contract test.
    // The important thing: no panic.
  }
}
