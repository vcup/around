# Feature Specification: Core Audio Playback Engine

**Feature Branch**: `001-core-playback-engine`

**Created**: 2026-05-26

**Status**: Draft

**Input**: User description: "Build the core audio playback engine for the around music player — format decoding, source abstraction, cross-platform output, background-first architecture, zero-overhead extension system."

## Clarifications

### Session 2026-05-26

- Q: Runtime-loaded extensions must have zero performance overhead vs. core code? → A: Yes — runtime-loaded extensions must perform identically to statically compiled core code. Floor is dynamic-linking level; target is full equivalence to static linkage. No per-call dispatch overhead is acceptable after initial load.
- Q: CLI process model — foreground blocking, detached, or foreground + IPC? → A: Foreground + local IPC. `around play <file>` runs playback in foreground (can be backgrounded with `&`). Separate `around pause/resume/seek` invocations communicate with the running engine via a local socket or named pipe.
- Q: Format detection mechanism — extension, magic bytes, or both? → A: Extension first, magic bytes fallback. Extension checked first (zero I/O). If unrecognized or absent, magic bytes inspected. If both disagree, magic bytes wins with a logged warning.
- Q: Extension discovery — explicit path, search paths, or config-driven? → A: Search paths + config-driven. Default search paths (`~/.local/share/around/decoders/`, system paths) with explicit path override. Config file can declare extension paths.
- Q: Configuration file format — TOML, KDL, JSON, YAML? → A: KDL. Node-based model naturally supports "simple default, unlimited depth" config philosophy. Use `knuffel` or `kdl-rs` parser crate.
- Q: Engine observability for MVP — none, structured logging, or full A7? → A: Structured logging only. Engine MUST emit `tracing` spans and events for pipeline stages (source open, decode start/end, output buffer fill, errors). Log level configurable via CLI flag. Metrics and profiling deferred.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Play a Local Audio File (Priority: P1)

A user has an audio file on their local filesystem. They invoke around with
the file path and hear the audio playback through their system's audio output.
The file plays from beginning to end, and the user can stop playback at any
time.

**Why this priority**: This is the fundamental value proposition. Without
playback, nothing else matters. Every other feature — libraries, streaming,
DSP, federation — builds on the ability to open a source, decode audio, and
send samples to an output device.

**Independent Test**: Run `around play /path/to/song.wav` and verify audio
output through the system speakers. Success means the complete pipeline
(source → decode → output) functions end-to-end for a single file.

**Acceptance Scenarios**:

1. **Given** a valid WAV file exists at a known path, **When** the user invokes
   `around play <path>`, **Then** audio output begins within 2 seconds and the
   file plays to completion.
2. **Given** playback is in progress, **When** the user sends a stop signal
   (Ctrl+C or equivalent), **Then** playback stops immediately and the process
   exits cleanly.
3. **Given** a file path that does not exist or is not a readable audio file,
   **When** the user invokes `around play <path>`, **Then** a clear error
   message is displayed and no audio output is attempted.

---

### User Story 2 - Basic Transport Controls (Priority: P2)

A user can control playback interactively — pause/resume, seek to a specific
position, skip forward/backward by fixed intervals, and query the current
playback state (position, duration, status).

**Why this priority**: Playback without control is a demo, not a player.
Control makes the engine usable for real listening sessions. This story also
validates that the IPC/command layer works bidirectionally.

**Independent Test**: Start playback, then use CLI commands to pause, resume,
seek, and query status. Verify each command produces the expected behavior
without restarting the engine.

**Acceptance Scenarios**:

1. **Given** a file is playing, **When** the user sends a pause command,
   **Then** playback pauses and resuming from the same position is possible.
2. **Given** a file is playing, **When** the user sends a seek command with a
   target timestamp, **Then** playback resumes from that position within 500ms.
3. **Given** a file is playing, **When** the user queries playback status,
   **Then** the response includes current position, total duration, and play
   state (playing/paused/stopped).

---

### User Story 3 - Extension-Loaded Format Decoder (Priority: P3)

A developer writes a format decoder as a standalone crate implementing the
`Decoder` trait, compiles it to a shared library, and loads it into around.
The engine discovers the new decoder and uses it to play files in that format
transparently.

**Why this priority**: The extension architecture is the constitutional
foundation (Core Principle III). Demonstrating that third-party decoders work
end-to-end proves the architecture works, even if the initial MVP ships with
built-in decoders for common formats. This story validates the extension
loading mechanism and trait contract.

**Independent Test**: Write a minimal decoder that handles a simple PCM-like
format, load it into around, play a file using that decoder. Verify the engine
routes the file to the correct decoder based on format detection.

**Acceptance Scenarios**:

1. **Given** a decoder extension is available at a known path, **When** the
   user loads it into around, **Then** the engine registers the decoder and
   lists it in the available format list.
2. **Given** a decoder for format X is loaded, **When** the user plays a file
   in format X, **Then** the engine routes decoding to the extension and
   playback succeeds.
3. **Given** a decoder extension is loaded and playback is in progress,
   **When** the user unloads the decoder, **Then** current playback (if using
   that decoder) stops gracefully with an error, and the decoder is removed
   from the registry.

---

### Edge Cases

- What happens when a source file is truncated or corrupted mid-playback?
- How does the engine handle a file whose format is detected but decoding
  fails (e.g., DRM-protected, unsupported codec variant)?
- What happens when the audio output device is disconnected during playback
  (e.g., headphones unplugged)?
- How does the engine respond when asked to play a file of zero duration or
  a format that produces no decodable samples?
- What happens when the user seeks beyond the file duration?
- How does the engine handle concurrent commands (e.g., rapid seek requests
  while playback is starting)?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST accept a local file path as a playback source and
  produce audio output through the system's default audio device.
- **FR-002**: System MUST detect the audio format of a given source and route
  it to the appropriate `Decoder` implementation. Detection uses file extension
  first (zero I/O), then falls back to magic byte inspection (file header
  signature). When extension and magic bytes disagree, magic bytes wins and a
  warning is logged.
- **FR-003**: System MUST support the WAV format as the sole built-in decoder.
  All other format support (FLAC, MP3, OGG Vorbis, Opus, AAC, etc.) is provided
  through loadable decoder extensions. The preferred codec backend for
  compressed formats is FFmpeg (libavcodec/libavformat).
- **FR-003a**: Extension loading MUST support compile-time bundling (static or
  dynamic linking into the binary) and runtime loading into the engine process.
  After loading, a runtime-loaded extension MUST exhibit identical per-call
  performance to the same code compiled statically into the core — no virtual
  dispatch, no indirection, no measurable throughput or latency difference. The
  minimum acceptable bound is parity with dynamic linking (dylib/.so/.dll
  symbol resolution); the target is full static-link equivalence.
- **FR-004**: System MUST expose play, pause, resume, stop, and seek commands
  through a CLI interface. Commands target a running engine process via a local
  IPC channel (socket or named pipe). `around play <file>` starts playback
  in the foreground; separate `around <command>` invocations control it.
- **FR-005**: System MUST expose a status query command returning current
  playback position, total duration, and playback state.
- **FR-006**: System MUST run the audio decoding and output pipeline on a
  separate thread from the command interface, with a local IPC channel
  (socket or named pipe) accepting commands from any around CLI process on
  the same machine. The CLI remains responsive during playback.
- **FR-007**: System MUST accept and load decoder extensions at runtime via
  a shared library loading mechanism. Extensions are discovered in default
  search paths (`~/.local/share/around/decoders/`, system-wide paths), may
  be explicitly loaded via a full path argument, and may be declared in the
  KDL configuration file. Loaded decoders MUST be queryable.
- **FR-008**: System MUST define and export a stable `Decoder` trait that
  extension authors implement, including methods for format detection,
  stream decoding, and metadata extraction.
- **FR-009**: System MUST define and export a stable `Source` trait that
  abstracts over data origins, with at minimum a local filesystem
  implementation.
- **FR-010**: System MUST handle source-open errors and decode errors
  gracefully — returning structured errors rather than panicking or crashing
  the engine thread.
- **FR-011**: System MUST compile and run on Linux, macOS, and Windows,
  producing audio output through each platform's native audio subsystem.
- **FR-012**: System MUST operate with zero network connectivity required for
  core playback (local files do not trigger any outbound connection).
- **FR-013**: System MUST achieve the performance targets defined in
  Constitution Directive A4: <10ms pipeline latency, <50MB idle memory, <5%
  CPU during playback, <500ms cold start.
- **FR-014**: System MUST accept configuration via a layered config system in
  KDL format: hardcoded defaults overridden by KDL config file overridden by
  CLI flags. Unknown keys MUST be rejected with a clear error at parse time.
- **FR-015**: System MUST emit structured log events via `tracing` for all
  pipeline stages: source open, decode start/end, output buffer fill, and all
  errors. Log level MUST be configurable via CLI flag (`--log-level`).
- **FR-016**: System MUST support two decoder loading paths to handle
  permission asymmetry between CLI and engine: (a) `load_decoder` — engine
  reads the shared library file directly using engine-level filesystem
  permissions; (b) `load_decoder_bytes` — CLI sends raw bytes, engine writes
  to temp file, loads, then deletes. Both produce functionally identical
  results at identical performance. The `ExtensionSource` enum MUST
  distinguish Builtin, Static, Dynamic, Runtime, Bytes, Config, and
  Discovered origins.

### Key Entities

- **Track**: Represents a playable audio item. Key attributes: source
  identifier (path/URL), detected `AudioFormat`, duration, `SampleSpec`
  (sample rate, channels, bit depth), `Metadata` (title, artist, album if
  available), `ContentType` tag.
- **Decoder**: A format-specific component that converts encoded audio bytes
  into raw PCM f32 samples. Key attributes: supported MIME types / format
  signatures, `SourceRequirements` (bitflags declaring what source
  capabilities are needed), capabilities (seeking, metadata extraction).
- **Source**: An abstraction over where audio bytes come from. Key attributes:
  byte stream from `open()`, `SourceCapabilities` (bitflags: MULTI_OPEN,
  SEEKABLE), content length (if known), content type hint, stable identifier.
- **PlaybackState**: The current state of the audio pipeline. Key attributes:
  position (milliseconds), duration, `PlaybackStatus` enum
  (Playing/Paused/Stopped), active decoder, active source.
- **SampleSpec**: Encoded audio parameters. All fields are pure numeric type
  aliases with well-known constants: `SampleRate` (`u32`, e.g. `SAMPLE_RATE_44100`),
  `ChannelLayout` (`u8`, e.g. `CHANNEL_STEREO`), `BitDepth` (`u8`, e.g. `BIT_DEPTH_16`).
  Zero overhead — no wrappers, no methods, identical to raw primitive operations.
- **ContentType**: String-keyed type tag with well-known `&str` constants
  (`"music"`, `"podcast"`, `"audiobook"`, `"radio"`, `"live_stream"`,
  `"ambient"`). User-extensible without modifying core.
- **ExtensionSource**: `u8` discriminant with constants (`SOURCE_BUILTIN`..`SOURCE_DISCOVERED`).
  Companion data (path, name) stored on ExtensionManifest.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A user with a WAV file can start playback within 2 seconds of
  invoking the command on consumer hardware.
- **SC-002**: Seek operations complete and audio resumes from the target
  position within 500ms.
- **SC-003**: The engine consumes under 50MB of resident memory at idle
  (process started, no playback).
- **SC-004**: Playback latency (from source read to audio output) is under
  10ms on consumer hardware with a local file source and PCM decoder.
- **SC-005**: A developer can implement a basic format decoder by implementing
  a single trait with no more than 6 required methods and load it into the
  running engine.
- **SC-006**: The engine does not crash or hang when presented with a
  corrupted, truncated, or unsupported audio file — it returns a descriptive
  error within 1 second.
- **SC-007**: The engine starts from cold boot (process launch to ready for
  commands) in under 500ms.
- **SC-008**: The engine process exits cleanly within 500ms of receiving a
  stop command during active playback.

## Assumptions

- The initial user interface is a CLI tool. GUI and TUI are deferred to
  subsequent features.
- Audio output defaults to the system's primary audio device. Device selection
  is deferred.
- The initial feature supports only local file sources. Remote sources (HTTP,
  network streams) are deferred.
- WAV is the only built-in decoder. All compressed format support is provided
  by extensions using FFmpeg as the preferred codec backend. Extensions can
  be compiled in statically, linked dynamically, or loaded at runtime — all
  three modes must exhibit identical call-site performance (no virtual dispatch
  overhead, parity with static linkage per FR-003a).
- The `Decoder` and `Source` traits are defined in a dedicated `around-core`
  crate that extension authors depend on.
- The engine itself is a library crate (`around-engine`) consumed by the CLI
  binary. The CLI binary runs playback in the foreground and exposes a local
  IPC channel (socket/named pipe) for control commands from other CLI
  invocations. The daemon model (fully separated engine process) is deferred.
- Configuration uses KDL format, parsed via `knuffel` or `kdl-rs`.
- Platform audio output uses existing Rust ecosystem crates where available
  (no new C dependencies for audio output unless unavoidable).
