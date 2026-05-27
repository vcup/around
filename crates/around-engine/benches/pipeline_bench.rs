//! Performance benchmarks for the around audio pipeline.
//!
//! Run with: cargo bench

use around_core::{DecoderFactory, Source};
use around_engine::{Engine, EngineConfig};
use around_source_file::FileSource;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::path::PathBuf;

fn fixture_path() -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    .join("examples")
    .join("example.wav")
}

fn bench_cold_start(c: &mut Criterion) {
  c.bench_function("cold_start", |b| {
    b.iter(|| {
      let config = EngineConfig::load();
      let engine = Engine::new(config);
      black_box(engine);
    });
  });
}

fn bench_pipeline_decode(c: &mut Criterion) {
  let config = EngineConfig::load();
  let engine = Engine::new(config);
  let source = FileSource::new(fixture_path());

  c.bench_function("pipeline_decode", |b| {
    b.iter(|| {
      let source = FileSource::new(fixture_path());
      let result = engine.play(Box::new(source));
      black_box(result);
    });
  });
}

fn bench_wav_decoder_open(c: &mut Criterion) {
  c.bench_function("wav_decoder_open", |b| {
    b.iter(|| {
      let source = FileSource::new(fixture_path());
      let result = around_codec_wav::WavDecoder::open(Box::new(source));
      black_box(result);
    });
  });
}

fn bench_file_source_open(c: &mut Criterion) {
  let source = FileSource::new(fixture_path());
  c.bench_function("file_source_open", |b| {
    b.iter(|| {
      let reader = source.open().unwrap();
      black_box(reader);
    });
  });
}

criterion_group!(
  benches,
  bench_cold_start,
  bench_pipeline_decode,
  bench_wav_decoder_open,
  bench_file_source_open
);
criterion_main!(benches);
