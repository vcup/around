# Data Model: Core Audio Playback Engine

**Feature**: 001-core-playback-engine
**Date**: 2026-05-26

## Entity Overview

```text
┌──────────┐     ┌──────────┐     ┌──────────────┐
│  Source  │────▶│ Decoder  │────▶│ Output Sink   │
└──────────┘     └──────────┘     └──────────────┘
       │                │                  │
       ▼                ▼                  ▼
  bytes in        PCM samples        audio device

                          ┌──────────────┐
                          │ PlaybackState│
                          └──────────────┘
                          ┌──────────────┐
                          │    Track     │
                          └──────────────┘
```

## Entity Definitions

### ContentType

String-tagged type identifier. User-extensible via any string, well-known values
provided as constants.

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentType(pub String);

impl ContentType {
    pub const MUSIC:       &'static str = "music";
    pub const PODCAST:     &'static str = "podcast";
    pub const AUDIOBOOK:   &'static str = "audiobook";
    pub const RADIO:       &'static str = "radio";
    pub const LIVE_STREAM: &'static str = "live_stream";
    pub const AMBIENT:     &'static str = "ambient";
}
```

### Track

Represents a playable audio item known to the engine.

| Field | Type | Description |
|-------|------|-------------|
| id | `TrackId` (newtype u64) | Unique identifier within the engine session |
| source | `Box<dyn Source>` | Where audio bytes come from |
| format | `AudioFormat` | Detected container/codec |
| duration | `Option<Duration>` | Total playback duration if known (None for streams) |
| sample_spec | `SampleSpec` | Sample rate, channels, bit depth |

**Validation**: `sample_spec.sample_rate > 0`, `sample_spec.channels in 1..=32`, `source` must be openable.

**State transitions**: None (immutable once loaded; replaced by a new Track on source change).

### BitDepth

Pure `u8` with well-known constants. No wrapper, no method — zero overhead by construction.

```rust
pub type BitDepth = u8;

pub const BIT_DEPTH_8:  BitDepth = 8;
pub const BIT_DEPTH_16: BitDepth = 16;
pub const BIT_DEPTH_24: BitDepth = 24;
pub const BIT_DEPTH_32: BitDepth = 32;
pub const BIT_DEPTH_64: BitDepth = 64;
```

Non-standard values (e.g., 20-bit) use the literal `20u8` directly.

### SampleRate

Pure `u32` with well-known constants.

```rust
pub type SampleRate = u32;

pub const SAMPLE_RATE_44100:  SampleRate = 44100;
pub const SAMPLE_RATE_48000:  SampleRate = 48000;
pub const SAMPLE_RATE_88200:  SampleRate = 88200;
pub const SAMPLE_RATE_96000:  SampleRate = 96000;
pub const SAMPLE_RATE_176400: SampleRate = 176400;
pub const SAMPLE_RATE_192000: SampleRate = 192000;
```

### ChannelLayout

Pure `u8` with well-known constants.

```rust
pub type ChannelLayout = u8;

pub const CHANNEL_MONO:        ChannelLayout = 1;
pub const CHANNEL_STEREO:      ChannelLayout = 2;
pub const CHANNEL_SURROUND_2_1: ChannelLayout = 3;
pub const CHANNEL_QUAD:        ChannelLayout = 4;
pub const CHANNEL_SURROUND_5_1: ChannelLayout = 6;
pub const CHANNEL_SURROUND_7_1: ChannelLayout = 8;
```

### PlaybackStatus

```rust
pub type PlaybackStatus = u8;

pub const PLAYING: PlaybackStatus = 0;
pub const PAUSED:  PlaybackStatus = 1;
pub const STOPPED: PlaybackStatus = 2;
```

### ExtensionSource

```rust
pub type ExtensionSource = u8;

pub const SOURCE_BUILTIN:    ExtensionSource = 0;
pub const SOURCE_STATIC:     ExtensionSource = 1;
pub const SOURCE_DYNAMIC:    ExtensionSource = 2;
pub const SOURCE_RUNTIME:    ExtensionSource = 3;
pub const SOURCE_BYTES:      ExtensionSource = 4;
pub const SOURCE_CONFIG:     ExtensionSource = 5;
pub const SOURCE_DISCOVERED: ExtensionSource = 6;
```

Path information for variants that have it (Runtime/Config/Discovered) and the name
for Bytes are stored alongside on `ExtensionManifest`, not in this tag.

### AudioFormat

Represents a detected audio format. All typed numeric fields use type aliases with constants.

| Field | Type | Description |
|-------|------|-------------|
| container | `String` | Container format name ("WAV", "MP3", "FLAC") |
| codec | `String` | Codec name ("PCM_S16LE", "MP3", "FLAC") |
| mime_type | `String` | MIME type ("audio/wav", "audio/mpeg") |
| sample_spec | `SampleSpec` | Sample rate, channels, bit depth |
| bitrate | `Option<u64>` | Bits per second if known |
