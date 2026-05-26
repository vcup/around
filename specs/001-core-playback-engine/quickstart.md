# Quickstart: Core Audio Playback Engine

## Prerequisites

- Rust stable (1.75+)
- Git
- Platform audio libraries:
  - Linux: `libasound2-dev` (ALSA) or `libpulse-dev` (PulseAudio)
  - macOS: Xcode Command Line Tools (CoreAudio built-in)
  - Windows: WASAPI (built-in)

## Build

```bash
git clone https://github.com/vcup/around
cd around
cargo build --release
```

## Run (Built-in WAV Only)

```bash
# Play a WAV file (foreground, Ctrl+C to stop)
./target/release/around play ./examples/example.wav

# In another terminal, control playback
./target/release/around pause
./target/release/around resume
./target/release/around status
./target/release/around seek --position 30
./target/release/around stop
```

## Run (With FFmpeg Extension)

```bash
# Build the FFmpeg decoder extension (requires ffmpeg dev libraries)
cargo build --release -p around-codec-ffmpeg

# Load it and play
./target/release/around load-decoder ./target/release/libaround_codec_ffmpeg.so
./target/release/around play ~/music/song.mp3
```

## Configuration

Create `~/.config/around/config.kdl`:

```kdl
log-level "info"

decoders {
    default-paths "~/.local/share/around/decoders"
    auto-load true
}

output {
    device "default"
}
```

## Run Tests

```bash
# All tests (unit + contract + integration)
cargo test

# Contract tests only
cargo test --test contract

# With logging visible
RUST_LOG=around_engine=trace cargo test -- --nocapture
```

## Verify Performance

```bash
# Build benchmarks (requires nightly)
cargo +nightly bench

# Key metrics to check:
# - pipeline_latency: must be < 10ms
# - cold_start: must be < 500ms
# - idle_memory: resident < 50MB
```
