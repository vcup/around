# Tasks: Core Audio Playback Engine

**Input**: Design documents from `specs/001-core-playback-engine/`

**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/

**Tests**: Constitution Core Principle — Test-First is NON-NEGOTIABLE. Contract tests for `Decoder` and `Source` traits are included. Integration tests validate end-to-end behavior per story.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Initialize the Rust workspace and all crate scaffolds

- [ ] T001 Create root Cargo.toml workspace manifest at /Cargo.toml with 7 members (around-core, around-engine, around-cli, around-codec-wav, around-codec-test-pcm, around-source-file, examples/simple_decoder)
- [ ] T002 [P] Create around-core crate scaffold (zero-dependency traits/type crate) at crates/around-core/Cargo.toml and crates/around-core/src/lib.rs
- [ ] T003 [P] Create around-engine crate scaffold (pipeline, IPC, extensions) at crates/around-engine/Cargo.toml and crates/around-engine/src/lib.rs
- [ ] T004 [P] Create around-cli crate scaffold (CLI binary) at crates/around-cli/Cargo.toml and crates/around-cli/src/main.rs
- [ ] T005 [P] Create around-codec-wav crate scaffold (built-in WAV decoder) at crates/around-codec-wav/Cargo.toml and crates/around-codec-wav/src/lib.rs
- [ ] T006 [P] Create around-codec-test-pcm crate scaffold (test PCM decoder) at crates/around-codec-test-pcm/Cargo.toml and crates/around-codec-test-pcm/src/lib.rs
- [ ] T007 [P] Create around-source-file crate scaffold (filesystem Source) at crates/around-source-file/Cargo.toml and crates/around-source-file/src/lib.rs
- [ ] T008 [P] Create test directory structure at tests/contract/, tests/integration/, and examples/
- [ ] T009 [P] Generate test WAV fixtures: a valid file at examples/example.wav, a truncated copy at examples/truncated.wav, and a zero-byte file at examples/zero.wav
- [ ] T010 Verify workspace compiles cleanly with `cargo build`

---

## Phase 2: Foundational — `around-core` Types & Traits

**Purpose**: Define all shared types, errors, bitflags, and the `Decoder`/`Source` trait contracts that every crate depends on

**⚠️ CRITICAL**: No crate may reference these types until this phase is complete

- [ ] T011 [P] Implement BitDepth, SampleRate, ChannelLayout type aliases with well-known constants in crates/around-core/src/types.rs
- [ ] T012 [P] Implement ContentType newtype with well-known string constants (MUSIC, PODCAST, AUDIOBOOK, RADIO, LIVE_STREAM, AMBIENT) in crates/around-core/src/types.rs
- [ ] T013 [P] Implement AudioFormat struct (container, codec, mime_type, sample_spec, bitrate) in crates/around-core/src/types.rs
- [ ] T014 [P] Implement SampleSpec struct (sample_rate, channels, bit_depth) with validation (rate>0, channels 1..=32) in crates/around-core/src/types.rs
- [ ] T015 [P] Implement ExtensionSource type alias with constants (SOURCE_BUILTIN through SOURCE_DISCOVERED) in crates/around-core/src/types.rs
- [ ] T016 [P] Implement PlaybackStatus type alias with constants (PLAYING, PAUSED, STOPPED) in crates/around-core/src/state.rs
- [ ] T017 [P] Implement Metadata struct (title, artist, album, duration, genre) in crates/around-core/src/metadata.rs
- [ ] T018 Implement AroundError enum with variants (FileNotFound, UnsupportedFormat, DecodeError, SourceIncompatible, NoTrack, DecoderLoadFailed, Internal, SourceAlreadyConsumed) in crates/around-core/src/error.rs
- [ ] T019 [P] Implement SourceCapabilities bitflags (NONE, MULTI_OPEN, SEEKABLE) in crates/around-core/src/source.rs
- [ ] T020 [P] Implement SourceRequirements bitflags (NONE, SEEKABLE, KNOWN_LENGTH, KNOWN_CONTENT_TYPE) and FormatSignature struct in crates/around-core/src/decoder.rs
- [ ] T021 Implement Source trait (capabilities, open, content_length, content_type, identifier) in crates/around-core/src/source.rs
- [ ] T022 Implement Decoder trait (supported_formats, source_requirements, can_decode, open, read, seek, metadata, output_format) in crates/around-core/src/decoder.rs
- [ ] T023 Wire module declarations (types, state, metadata, error, source, decoder) and export all public items in crates/around-core/src/lib.rs
- [ ] T024 Verify around-core compiles with `cargo build -p around-core`

**Checkpoint**: Foundation ready — all types and trait contracts defined. User story implementation can now begin.

---

## Phase 3: User Story 1 — Play a Local Audio File (Priority: P1) 🎯 MVP

**Goal**: User runs `around play /path/to/song.wav` and hears audio output through system speakers. The complete Source → Decoder → Output pipeline works end-to-end for WAV files.

**Independent Test**: Run `around play examples/example.wav` — audio plays to completion, Ctrl+C stops cleanly. Missing/corrupt files produce descriptive errors.

### Contract Tests for User Story 1 (write FIRST, ensure they FAIL)

- [ ] T025 [P] [US1] Write Decoder trait contract tests in tests/contract/decoder.rs — verify format honesty, source requirements gating, idempotent detection, open-or-fail, f32 PCM output, seek contract, metadata availability, no-panic on corrupt stream, using test fixtures from examples/
- [ ] T026 [P] [US1] Write Source trait contract tests in tests/contract/source.rs — verify open-or-fail, length honesty, MULTI_OPEN semantics, SEEKABLE capability, no I/O in metadata methods, identifier stability, second-open rejection for single-shot sources, using test fixtures from examples/

### Implementation for User Story 1

- [ ] T027 [US1] Implement local filesystem Source with MULTI_OPEN | SEEKABLE capabilities in crates/around-source-file/src/lib.rs
- [ ] T028 [US1] Implement WAV decoder using symphonia (implements Decoder trait, SourceRequirements::SEEKABLE | KNOWN_LENGTH, PCM→f32 conversion) in crates/around-codec-wav/src/lib.rs
- [ ] T029 [US1] Implement format detection function (extension-first, magic bytes fallback, magic-bytes-wins-with-warning) in crates/around-engine/src/config.rs
- [ ] T030 [US1] Implement audio pipeline engine (Source::open → decoder selection → Decoder::read → cpal output callback, separate OS thread for pipeline) in crates/around-engine/src/pipeline.rs
- [ ] T031 [US1] Implement KDL layered config loading (compiled defaults → ~/.config/around/config.kdl → CLI overrides, unknown-key rejection via knuffel) in crates/around-engine/src/config.rs
- [ ] T032 [US1] Implement platform audio output backends via cpal (mod.rs + linux.rs/macos.rs/windows.rs stubs) in crates/around-engine/src/platform/
- [ ] T033 [US1] Implement `around play <path>` CLI subcommand using clap in crates/around-cli/src/main.rs
- [ ] T034 [US1] Implement Ctrl+C signal handler (graceful stop, clean process exit within 500ms per SC-008) in crates/around-cli/src/main.rs
- [ ] T035 [US1] Implement structured tracing spans/events (source_open, decode_start, decode_end, output_buffer_fill, errors) with configurable RUST_LOG level in crates/around-engine/src/pipeline.rs
- [ ] T036 [US1] Write integration test for end-to-end WAV playback pipeline in tests/integration/playback_pipeline.rs — verify play to completion, missing file error, corrupt file error

**Checkpoint**: User Story 1 fully functional — `around play example.wav` works end-to-end. Contract tests pass. Integration test passes.

---

## Phase 4: User Story 2 — Basic Transport Controls (Priority: P2)

**Goal**: User controls playback via separate CLI invocations — `around pause`, `around resume`, `around seek --position 30`, `around status`. IPC channel carries JSON commands between CLI and engine.

**Independent Test**: Open terminal 1: `around play example.wav`. Open terminal 2: `around pause`, `around resume`, `around status`, `around seek --position 5`, `around stop`. Each command produces correct audio behavior without restarting the engine.

### Implementation for User Story 2

- [ ] T037 [US2] Implement IPC server (Unix domain socket on Linux/macOS, named pipe on Windows) with tokio current_thread runtime in crates/around-engine/src/ipc.rs — accept connections, parse newline-delimited JSON commands, return JSON responses
- [ ] T038 [US2] Implement PlaybackState tracking struct (position, duration, status, active track ref) in crates/around-engine/src/lib.rs
- [ ] T039 [US2] Implement IPC command handlers for play, pause, resume, stop with state transitions in crates/around-engine/src/ipc.rs
- [ ] T040 [US2] Implement IPC command handlers for seek (position validation, decoder re-seek) and status query (position, duration, playback state, track info) in crates/around-engine/src/ipc.rs
- [ ] T041 [US2] Implement IPC command handler for list_decoders (return registered decoders with name, formats, source) in crates/around-engine/src/ipc.rs
- [ ] T042 [US2] Implement around-cli transport subcommands (pause, resume, seek, stop, status) that connect to engine IPC socket in crates/around-cli/src/main.rs
- [ ] T043 [US2] Implement `--log-level` CLI flag (error/warn/info/debug/trace) mapped to RUST_LOG directive in crates/around-cli/src/main.rs
- [ ] T044 [US2] Write integration test for IPC transport control flow in tests/integration/ipc_control.rs — verify play→pause→resume→seek→status→stop sequence

**Checkpoint**: US1 + US2 work independently. Foreground playback and IPC control both functional.

---

## Phase 5: User Story 3 — Extension-Loaded Format Decoder (Priority: P3)

**Goal**: A decoder compiled as a shared library can be loaded at runtime and used transparently. Both `load_decoder` (path-based) and `load_decoder_bytes` (bytes-based) loading paths work. Extension performance matches static linkage.

**Independent Test**: Build `around-codec-test-pcm` as a .so/.dylib/.dll. Load it via `around load-decoder <path>`. Play a test PCM file using it. Verify the decoder appears in `around list-decoders`. Load via bytes mode and verify identical results.

### Implementation for User Story 3

- [ ] T045 [US3] Implement extension manager (decoder registry, libloading-based load/unload, symbol resolution for `create_decoder` FFI entry point) in crates/around-engine/src/extensions.rs
- [ ] T046 [US3] Implement load_decoder IPC handler — engine reads shared library file via engine's filesystem permissions, resolves symbols, registers decoder in crates/around-engine/src/ipc.rs
- [ ] T047 [US3] Implement load_decoder_bytes IPC handler — CLI sends base64-encoded bytes, engine writes to temp file, loads via libloading, deletes temp, registers decoder in crates/around-engine/src/ipc.rs
- [ ] T048 [US3] Implement extension discovery (scan default search paths ~/.local/share/around/decoders/, system paths from KDL config) in crates/around-engine/src/extensions.rs
- [ ] T049 [US3] Implement test PCM decoder in crates/around-codec-test-pcm/src/lib.rs — minimal Decoder impl that handles a simple raw PCM format, exports `create_decoder` FFI symbol
- [ ] T050 [US3] Implement around-cli load-decoder (path) and list-decoders subcommands in crates/around-cli/src/main.rs
- [ ] T051 [US3] Write contract test for extension loading (verify load via path, load via bytes, duplicate load rejection, unload, post-unload cleanup) in tests/contract/extensions.rs
- [ ] T052 [US3] Write integration test for extension-loaded end-to-end playback in tests/integration/extension_playback.rs — load test PCM decoder, play test PCM file, verify audio output

**Checkpoint**: All three user stories independently functional. Extension architecture validated.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Performance validation, code quality, and final checks

- [ ] T053 [P] Implement performance benchmarks — pipeline_latency (<10ms), cold_start (<500ms), idle_rss (<50MB), cpu_usage (<5%) — in benches/pipeline_bench.rs
- [ ] T054 Run `cargo fmt` and `cargo clippy` across the workspace, fix all warnings and errors
- [ ] T055 Run `cargo test` — all contract tests, integration tests, and unit tests pass
- [ ] T056 Run `cargo build --release` and verify stripped binary size <10MB (Linux headless)
- [ ] T057 Validate quickstart.md walkthrough — build, play WAV, run contract tests, configure KDL, all steps succeed
- [ ] T058 Error handling audit — verify no panics in pipeline path, corrupt files return AroundError::DecodeError, device disconnect returns graceful error, concurrent commands don't crash engine

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **User Story 1 (Phase 3)**: Depends on Foundational — no dependency on other stories
- **User Story 2 (Phase 4)**: Depends on Foundational + US1 (requires running engine with pipeline) — may begin after US1 checkpoint
- **User Story 3 (Phase 5)**: Depends on Foundational + US2 (requires IPC for load_decoder commands) — may begin after US2 checkpoint
- **Polish (Phase 6)**: Depends on all desired stories being complete

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

- **Phase 1**: All crate scaffold tasks (T002–T009) are [P] — can create all 7 crates + tests + fixtures in parallel
- **Phase 2**: All type/constant definition tasks (T011–T017) are [P] — can define all types simultaneously; T019–T020 (bitflags) are [P] with each other
- **Phase 3**: Contract tests T025 and T026 are [P] — write both in parallel before any implementation
- **Phase 6**: Formatting, benchmarks, and audit tasks are independent [P]

---

## Parallel Example: Phase 1 Setup

```bash
# Launch all crate scaffolds together:
Task: "Create around-core crate scaffold at crates/around-core/"
Task: "Create around-engine crate scaffold at crates/around-engine/"
Task: "Create around-cli crate scaffold at crates/around-cli/"
Task: "Create around-codec-wav crate scaffold at crates/around-codec-wav/"
Task: "Create around-codec-test-pcm crate scaffold at crates/around-codec-test-pcm/"
Task: "Create around-source-file crate scaffold at crates/around-source-file/"
Task: "Create test directory structure at tests/contract/, tests/integration/"
Task: "Generate test WAV fixtures at examples/"
```

## Parallel Example: Phase 2 Foundational Types

```bash
# Launch all independent type definitions together:
Task: "Implement BitDepth, SampleRate, ChannelLayout in crates/around-core/src/types.rs"
Task: "Implement ContentType with well-known constants in crates/around-core/src/types.rs"
Task: "Implement AudioFormat struct in crates/around-core/src/types.rs"
Task: "Implement SampleSpec struct in crates/around-core/src/types.rs"
Task: "Implement ExtensionSource type alias in crates/around-core/src/types.rs"
Task: "Implement PlaybackStatus in crates/around-core/src/state.rs"
Task: "Implement Metadata struct in crates/around-core/src/metadata.rs"
Task: "Implement SourceCapabilities bitflags in crates/around-core/src/source.rs"
Task: "Implement SourceRequirements bitflags in crates/around-core/src/decoder.rs"
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
5. Each story adds value without breaking previous stories

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
   - Developer B: Polish phase (benchmarks, audit)

---

## Notes

- [P] tasks = different files, no dependencies — run in parallel
- [US1]/[US2]/[US3] labels map tasks to specific user stories for traceability
- Constitution mandates test-first for trait contracts; contract tests MUST fail before implementation
- Each user story checkpoint represents an independently testable increment
- After T056, the headless binary must be <10MB stripped per A4 constraints
- Symphonia should be added with only `wav` feature enabled initially (reduce compile time)
- All newtype/alias types follow `#[repr(transparent)]` / zero-overhead patterns per spec
- KDL config must reject unknown keys at parse time via knuffel's typed decoding
- FFmpeg extension crate (`around-codec-ffmpeg`) is deferred to a subsequent feature
