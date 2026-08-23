# Tasks: Core Audio Playback Engine

**Input**: Design documents from `specs/001-core-playback-engine/`

**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

**Tests**: Constitution Core Principle — Test-First is NON-NEGOTIABLE. Contract tests for `Decoder` and `Source` traits are included. Integration tests validate end-to-end behavior per story.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Initialize the Rust workspace and all crate scaffolds

- [X] T001 Create root Cargo.toml workspace manifest at /Cargo.toml with 7 members (around-core, around-engine, around-cli, around-codec-wav, around-codec-test-pcm, around-source-file, examples/simple_decoder)
- [X] T002 [P] Create around-core crate scaffold (zero-dependency traits/type crate) at crates/around-core/Cargo.toml and crates/around-core/src/lib.rs
- [X] T003 [P] Create around-engine crate scaffold (pipeline, IPC, extensions) at crates/around-engine/Cargo.toml and crates/around-engine/src/lib.rs
- [X] T004 [P] Create around-cli crate scaffold (CLI binary) at crates/around-cli/Cargo.toml and crates/around-cli/src/main.rs
- [X] T005 [P] Create around-codec-wav crate scaffold (built-in WAV decoder) at crates/around-codec-wav/Cargo.toml and crates/around-codec-wav/src/lib.rs
- [X] T006 [P] Create around-codec-test-pcm crate scaffold (test PCM decoder) at crates/around-codec-test-pcm/Cargo.toml and crates/around-codec-test-pcm/src/lib.rs
- [X] T007 [P] Create around-source-file crate scaffold (filesystem Source) at crates/around-source-file/Cargo.toml and crates/around-source-file/src/lib.rs
- [X] T008 [P] Create test directory structure at tests/contract/, tests/integration/, and examples/
- [X] T009 [P] Generate test WAV fixtures: a valid file at examples/example.wav, a truncated copy at examples/truncated.wav, and a zero-byte file at examples/zero.wav
- [X] T010 Verify workspace compiles cleanly with `cargo build`

---

## Phase 2: Foundational — `around-core` Types & Traits

**Purpose**: Define all shared types, errors, bitflags, and the `Decoder`/`Source` trait contracts that every crate depends on

**⚠️ CRITICAL**: No crate may reference these types until this phase is complete

- [X] T011 [P] Implement `SampleRate`, `ChannelLayout`, `PcmEncoding`, and `ByteOrder` types with well-known rate/channel constants in `crates/around-core/src/types.rs`.
- [X] T012 [P] Implement ContentType newtype with well-known string constants (MUSIC, PODCAST, AUDIOBOOK, RADIO, LIVE_STREAM, AMBIENT) in crates/around-core/src/types.rs
- [X] T014 [P] Implement complete `SampleSpec` (`sample_rate`, `channels`, `PcmEncoding`, `ByteOrder`, `interleave`) with validation in `crates/around-core/src/types.rs`.
- [X] T015 [P] Implement ExtensionSource type alias with constants (SOURCE_BUILTIN through SOURCE_DISCOVERED) in crates/around-core/src/types.rs
- [X] T016 [P] Implement PlaybackStatus type alias with constants (PLAYING=0, PAUSED=1, STOPPED=2, BUFFERING=3) in crates/around-core/src/state.rs
- [X] T017 [P] Implement Metadata struct (title, artist, album, duration, genre) in crates/around-core/src/metadata.rs
- [X] T018 Implement AroundError enum with variants (FileNotFound, UnsupportedFormat, DecodeError, SourceIncompatible, NoTrack, DecoderLoadFailed, Internal, SourceAlreadyConsumed, InvalidPosition, CodecNotSupported) in crates/around-core/src/error.rs
- [X] T019 [P] Implement SourceCapabilities bitflags (NONE, MULTI_OPEN, SEEKABLE) in crates/around-core/src/source.rs
- [X] T020 [P] Implement SourceRequirements bitflags (NONE, SEEKABLE, KNOWN_LENGTH, KNOWN_CONTENT_TYPE) and FormatSignature struct in crates/around-core/src/decoder.rs
- [X] T021 Implement Source trait (capabilities, open, content_length, content_type, identifier) in crates/around-core/src/source.rs
- [X] T022 Implement Decoder trait (Decoder: read/seek/metadata/output_format; DecoderFactory: supported_formats/source_requirements/can_decode/open) in crates/around-core/src/decoder.rs. seek() doc: "sample frame offset — engine converts ms to sample frames."
- [X] T023 Wire module declarations (types, state, metadata, error, source, decoder) and export all public items in crates/around-core/src/lib.rs
- [X] T024 Verify around-core compiles with `cargo build -p around-core`

**Checkpoint**: Foundation ready — all types and trait contracts defined. User story implementation can now begin.

---

## Phase 3: User Story 1 — Play a Local Audio File (Priority: P1) 🎯 MVP

**Goal**: User runs `around play /path/to/song.wav` and hears audio output through system speakers. The complete Source → Decoder → Output pipeline works end-to-end for WAV files.

**Independent Test**: Run `around play examples/example.wav` — audio plays to completion, Ctrl+C stops cleanly. Missing/corrupt files produce descriptive errors.

### Contract Tests for User Story 1 (write FIRST, ensure they FAIL)

- [X] T025 [P] [US1] Write Decoder trait contract tests in tests/contract/decoder.rs — verify format honesty, source requirements gating, idempotent detection, open-or-fail, f32 PCM output, seek contract, metadata availability, no-panic on corrupt stream, using test fixtures from examples/
- [X] T026 [P] [US1] Write Source trait contract tests in tests/contract/source.rs — verify open-or-fail, length honesty, MULTI_OPEN semantics, SEEKABLE capability, no I/O in metadata methods, identifier stability, second-open rejection for single-shot sources, using test fixtures from examples/

### Implementation for User Story 1

- [X] T027 [US1] Implement local filesystem Source with MULTI_OPEN | SEEKABLE capabilities in crates/around-source-file/src/lib.rs
- [X] T028 [US1] Implement WAV decoder using symphonia (implements Decoder trait, SourceRequirements::SEEKABLE | KNOWN_LENGTH, PCM→f32 conversion) in crates/around-codec-wav/src/lib.rs
- [X] T029 [US1] Implement format detection function (extension-first, magic bytes fallback, magic-bytes-wins-with-warning) in crates/around-engine/src/config.rs
- [X] T030 [US1] Implement audio pipeline engine (Source::open → decoder selection → Decoder::read → cpal output callback, separate OS thread for pipeline) in crates/around-engine/src/pipeline.rs. Decode errors: skip damaged frame, warn, continue; stop after 3 consecutive errors.
- [X] T031 [US1] Implement KDL layered config loading (compiled defaults → ~/.config/around/config.kdl → CLI overrides) in crates/around-engine/src/config.rs. Use `kdl-rs` for manual tree traversal; strict validation of core keys; `[extensions]` node passed through.
- [X] T032 [US1] Implement platform audio output backends via cpal (mod.rs + linux.rs/macos.rs/windows.rs stubs) in crates/around-engine/src/platform/
- [X] T033 [US1] Implement `around play <path>` CLI subcommand using clap in crates/around-cli/src/main.rs
- [X] T034 [US1] Implement signal handler (SIGINT, SIGHUP, SIGTERM) — graceful stop, clean process exit within 500ms per SC-008 in crates/around-cli/src/main.rs
- [X] T035 [US1] Implement structured tracing spans/events (source_open, decode_start, decode_end, output_buffer_fill, decode_skip, errors) with configurable RUST_LOG level in crates/around-engine/src/pipeline.rs
- [X] T036 [US1] Write integration test for end-to-end WAV playback pipeline in tests/integration/playback_pipeline.rs — verify play to completion, missing file error, corrupt file error

**Checkpoint**: User Story 1 fully functional — `around play example.wav` works end-to-end. Contract tests pass. Integration test passes.

---

## Phase 4: User Story 2 — Basic Transport Controls (Priority: P2)

**Goal**: User controls playback via separate CLI invocations — `around pause`, `around resume`, `around seek --position 30`, `around status`. IPC channel carries JSON commands between CLI and engine.

**Independent Test**: Open terminal 1: `around play example.wav`. Open terminal 2: `around pause`, `around resume`, `around status`, `around seek --position 5`, `around stop`. Each command produces correct audio behavior without restarting the engine.

### Implementation for User Story 2

- [X] T037 [US2] Implement IPC server (Unix domain socket on Linux/macOS, named pipe on Windows) with tokio current_thread runtime in crates/around-engine/src/ipc.rs — accept connections, parse newline-delimited JSON commands, return JSON responses. Socket lifecycle: 0600 permissions, connect-check-before-bind on start, delete stale socket if unresponsive.
- [X] T038 [US2] Implement PlaybackState tracking struct (position, duration, status — Playing/Paused/Stopped/Buffering, active track ref) in crates/around-engine/src/lib.rs
- [X] T039 [US2] Implement IPC command handlers for play, pause, resume, stop with state transitions in crates/around-engine/src/ipc.rs. All commands serialized FIFO. Play-while-playing: replace current track. State transitions: Stop→Play (new track), Paused→Resume→Playing, Playing→Seek→Buffering→Playing.
- [X] T040 [US2] Implement IPC command handlers for seek (position validation, decoder re-seek, INVALID_POSITION error) and status query (position, duration, playback state including Buffering, track info, device status) in crates/around-engine/src/ipc.rs
- [X] T041 [US2] Implement IPC command handler for list_decoders (return registered decoders with name, formats, source) in crates/around-engine/src/ipc.rs
- [X] T042 [US2] Implement around-cli transport subcommands (pause, resume, seek, stop, status) that connect to engine IPC socket in crates/around-cli/src/main.rs
- [X] T043 [US2] Implement `--log-level` CLI flag (error/warn/info/debug/trace) mapped to RUST_LOG directive in crates/around-cli/src/main.rs
- [X] T044 [US2] Write integration test for IPC transport control flow in tests/integration/ipc_control.rs — verify play→pause→resume→seek→status→stop sequence
- [X] T059 [US2] Implement device disconnect detection and auto-pause in crates/around-engine/src/pipeline.rs — detect cpal stream error, transition to Paused, report device loss via IPC status. Add configurable recovery behavior (auto-resume vs. manual) in crates/around-engine/src/config.rs.
- [X] T060 [P] [US2] Implement IPC cleanup command handler in crates/around-engine/src/ipc.rs — scan and remove stale temp files from aborted decoder loads. Add `cleanup` CLI subcommand in crates/around-cli/src/main.rs.

**Checkpoint**: US1 + US2 work independently. Foreground playback and IPC control both functional.

---

## Phase 5: User Story 3 — Extension-Loaded Format Decoder (Priority: P3)

**Goal**: A decoder compiled as a shared library can be loaded at runtime and used transparently. Both `load_decoder` (path-based) and `load_decoder_bytes` (bytes-based) loading paths work. Extension performance matches static linkage.

**Independent Test**: Build `around-codec-test-pcm` as a .so/.dylib/.dll. Load it via `around load-decoder <path>`. Play a test PCM file using it. Verify the decoder appears in `around list-decoders`. Load via bytes mode and verify identical results.

### Implementation for User Story 3

- [X] T045 [US3] Implement extension manager (decoder registry, libloading-based load/unload, symbol resolution for `create_decoder` FFI entry point) in crates/around-engine/src/extensions.rs
- [X] T046 [US3] Implement load_decoder IPC handler — engine reads shared library file via engine's filesystem permissions, resolves symbols, registers decoder in crates/around-engine/src/ipc.rs. Duplicate load: same path + version → idempotent (return existing); same path + different version → replace (unload old, load new).
- [X] T047 [US3] Implement load_decoder_bytes IPC handler — CLI sends base64-encoded bytes, engine writes to temp file, loads via libloading, deletes temp, registers decoder in crates/around-engine/src/ipc.rs
- [X] T048 [US3] Implement extension discovery (scan default search paths: `~/.local/share/around/decoders/`, `/usr/lib/around/decoders/` on Linux, `/Library/Application Support/around/decoders/` on macOS, `%PROGRAMDATA%\around\decoders\` on Windows) in crates/around-engine/src/extensions.rs. Lazy-load: discovery deferred to first `list_decoders` or `play` command.
- [X] T049 [US3] Implement test PCM decoder in crates/around-codec-test-pcm/src/lib.rs — minimal Decoder impl that handles a simple raw PCM format, exports `create_decoder` FFI symbol
- [X] T050 [US3] Implement around-cli load-decoder (path) and list-decoders subcommands in crates/around-cli/src/main.rs
- [X] T051 [US3] Write contract test for extension loading (verify load via path, load via bytes, duplicate load idempotent, duplicate load with version change replaces, unload, post-unload cleanup) in tests/contract/extensions.rs
- [X] T052 [US3] Write integration test for extension-loaded end-to-end playback in tests/integration/extension_playback.rs — load test PCM decoder, play test PCM file, verify audio output
- [X] T061 [US3] Implement decoder fallback logic in crates/around-engine/src/config.rs — when selected decoder fails to open(), attempt: (1) all decoders claiming support for detected format in registration order, (2) all other decoders in registration order. First successful open wins. Update format detection T029 to call fallback chain.

**Checkpoint**: All three user stories independently functional. Extension architecture validated.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Performance validation, code quality, and final checks

- [X] T053 [P] Implement performance benchmarks — internal pipeline latency, cold_start (<500ms, excl. extension discovery), idle_rss (<50MB, Linux /proc/self/statm, 10 samples median, 30s idle), cpu_usage (<5%) — in benches/pipeline_bench.rs
- [X] T054 Run `cargo fmt` and `cargo clippy` across the workspace, fix all warnings and errors
- [X] T055 Run `cargo test` — all contract tests, integration tests, and unit tests pass
- [X] T056 Run `cargo build --release` and verify stripped binary size <10MB (Linux headless)
- [X] T057 Validate quickstart.md walkthrough — build, play WAV, run contract tests, configure KDL, all steps succeed
- [X] T058 Error handling audit — verify: no panics in pipeline path, corrupt files return AroundError::DecodeError with VLC skip (up to 3 consecutive), device disconnect returns graceful error and auto-pause, concurrent commands serialized FIFO, DRM/codec-variant returns CodecNotSupported, seek-beyond-duration returns InvalidPosition. Update tests to cover new error variants and behaviors.

---

## Phase 7: Spec Compliance Update (Requirements Quality Review)

**Purpose**: Implement remaining changes from the 2026-05-27 requirements quality review. Tasks T059–T066 correspond to spec changes not yet reflected in implementation.

- [X] T059 [US2] Implement device disconnect detection and auto-pause in crates/around-engine/src/pipeline.rs — detect cpal stream error via StreamError callback, transition playback to Paused, report device loss via IPC status. Add configurable recovery behavior (auto-resume on reconnect vs. require manual resume) as KDL config option `output.auto_reconnect` in crates/around-engine/src/config.rs.
- [X] T060 [P] [US2] Implement IPC `cleanup` command handler in crates/around-engine/src/ipc.rs — scan system temp directory for `around-decoder-*` prefixed files, remove stale entries. Return list of removed files in JSON response. Add `around cleanup` CLI subcommand in crates/around-cli/src/main.rs.
- [X] T061 [US3] Implement decoder fallback logic in crates/around-engine/src/config.rs — when selected decoder fails to open(), attempt fallback chain per FR-002: (1) same-format decoders in registration order, (2) all other decoders in registration order. First successful open wins. Wire into `Engine::play()` in crates/around-engine/src/pipeline.rs.
- [X] T062 [P] [US1] Migrate KDL config parsing from `knuffel` to `kdl-rs` in crates/around-engine/src/config.rs. Replace `#[derive(knuffel::Decode)]` with manual KDL document traversal. Implement: strict validation of known core keys (reject unknown), passthrough of `[extensions]` node children to extension manager. Update Cargo.toml dependency: remove `knuffel`, add `kdl-rs`.
- [X] T063 [P] [US1] Update signal handling in crates/around-cli/src/main.rs — extend existing SIGINT (Ctrl+C) handler to also handle SIGHUP and SIGTERM with the same cleanup sequence: stop playback, close IPC socket, release audio device, delete socket file, exit cleanly.
- [X] T064 [P] [US2] Implement IPC socket lifecycle in crates/around-engine/src/ipc.rs — set socket permissions to 0600 after bind. On engine start, if socket file exists: attempt UnixStream::connect; if successful (another instance running), return error and exit; if connection refused, delete stale socket and bind. Update engine startup in crates/around-engine/src/lib.rs.
- [X] T065 [US2] Update PlaybackState and IPC status to include BUFFERING state in crates/around-engine/src/lib.rs and crates/around-engine/src/ipc.rs — during source open, decoder init, and seek operations, set status to Buffering. Status query returns `"state": "buffering"`. On completion, transition to previous state (Playing or Paused).
- [X] T066 [P] Update integration and contract tests for new behaviors:
  - tests/contract/extensions.rs: test duplicate load idempotent + version-change-replace
  - tests/integration/ipc_control.rs: test play-while-playing replace, cleanup command, BUFFERING status during seek
  - tests/integration/playback_pipeline.rs: test decoder fallback, VLC skip (corrupt mid-file), signal handling (SIGHUP/SIGTERM equivalent)
  - tests/contract/decoder.rs: test CodecNotSupported error, InvalidPosition error, seek sample-frame unit

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **User Story 1 (Phase 3)**: Depends on Foundational — no dependency on other stories
- **User Story 2 (Phase 4)**: Depends on Foundational + US1 (requires running engine with pipeline)
- **User Story 3 (Phase 5)**: Depends on Foundational + US2 (requires IPC for load_decoder commands)
- **Polish (Phase 6)**: Depends on all desired stories being complete
- **Spec Compliance (Phase 7)**: Depends on Phase 6 completion — applies cross-cutting spec updates

### User Story Dependencies

- **User Story 1 (P1)**: Can start after Phase 2 — no dependency on US2/US3
- **User Story 2 (P2)**: Depends on US1 (needs running engine pipeline to control)
- **User Story 3 (P3)**: Depends on US2 (needs IPC layer for load_decoder commands)

### Within Each User Story

- Contract tests written FIRST, verified to FAIL, then pass after implementation
- Models/types before services/pipeline
- Core implementation before integration
- Story complete (all tests pass) before moving to next priority

### Parallel Opportunities

- **Phase 7**: T060 (cleanup), T062 (kdl-rs), T063 (signals), T064 (socket lifecycle), T066 (tests) are all [P] — different files, can run in parallel
- T059 (device disconnect) → depends on T062 (kdl-rs config) for the config option
- T061 (decoder fallback) → depends on T062 (kdl-rs) for the select_decoder function
- T065 (BUFFERING state) → depends on T016 (BUFFERING constant already added)

---

## Parallel Example: Phase 7 Spec Compliance

```bash
# Launch independent tasks together:
Task: "Implement IPC cleanup command (T060)"
Task: "Migrate KDL config to kdl-rs (T062)"
Task: "Update signal handling for SIGHUP/SIGTERM (T063)"
Task: "Implement IPC socket lifecycle (T064)"
Task: "Update integration and contract tests (T066)"
# Then sequentially:
Task: "Implement device disconnect (T059)" — depends on T062
Task: "Implement decoder fallback (T061)" — depends on T062
Task: "Update PlaybackState BUFFERING (T065)" — depends on T064
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup — all crates scaffolded, workspace compiles
2. Complete Phase 2: Foundational — `around-core` types and traits defined, compiles
3. Complete Phase 3: User Story 1 — `around play example.wav` works
4. **STOP and VALIDATE**: Contract tests pass, integration test passes, audio plays
5. This is the MVP — demonstrable core playback engine

### Incremental Delivery

1. Setup + Foundational → `around-core` crate published, trait contracts stable
2. Add User Story 1 → `around play file.wav` works → **MVP shipped**
3. Add User Story 2 → `around pause/resume/seek/status` works → interactive playback
4. Add User Story 3 → `around load-decoder` + extension playback → third-party format support
5. Phase 7 → All spec compliance gaps closed

### Parallel Team Strategy

With multiple developers:

1. Team completes Setup + Foundational together (serial dependency)
2. Once Foundational is done:
   - Developer A: User Story 1 (around-source-file, around-codec-wav, pipeline, CLI play)
   - Developer B: Prepares User Story 2 (reads IPC contracts, plans state machine) while US1 completes
3. Once US1 is done:
   - Developer A: User Story 2 (IPC server, transport commands)
   - Developer B: User Story 3 prep (extension contracts, test PCM decoder)
4. Once US2 is done:
   - Developer A: User Story 3 (extension manager, load paths)
   - Developer B: Phase 7 tasks (kdl-rs migration, signal handling, tests)

---

## Notes

- [P] tasks = different files, no dependencies — run in parallel
- [US1]/[US2]/[US3] labels map tasks to specific user stories for traceability
- Constitution mandates test-first for trait contracts; contract tests MUST fail before implementation
- Each user story checkpoint represents an independently testable increment
- After T056, the headless binary must be <10MB stripped per A4 constraints
- Symphonia should be added with only `wav` feature enabled initially (reduce compile time)
- All newtype/alias types follow `#[repr(transparent)]` / zero-overhead patterns per spec
- KDL config uses `kdl-rs` for manual tree traversal (migrated from `knuffel` per FR-014 requirements review)
- FFmpeg extension crate (`around-codec-ffmpeg`) is deferred to a subsequent feature
- Phase 7 tasks (T059–T066) address spec changes from 2026-05-27 requirements quality review (see checklists/requirements.md)
- Decoder trait is split: Decoder (4 object-safe methods) + DecoderFactory (4 non-object-safe methods) = 8 total methods
- SC-004 (latency) is best-effort, not a hard target; SC-005 updated to 8 methods; SC-007 excludes extension discovery
