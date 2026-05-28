# Tasks: MVP Audio Output Pipeline & Critical Fixes

**Input**: Design documents from `specs/002-mvp-audio-output/`

**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

**Tests**: Test tasks are included per spec US3 requirement for integration test coverage and per constitution Test-First mandate.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Verify 001 workspace state and generate test fixtures

- [ ] T001 Verify workspace compiles cleanly with `cargo build --workspace`
- [ ] T002 Verify existing test suite passes with `cargo test --workspace`
- [ ] T003 Use ffmpeg to generate test WAV fixtures at examples/:
  - `examples/long-test.wav`: 5s, 8000Hz, 8-bit uPCM mono (~40KB) — 5s IPC window
  - `examples/example.wav`: 1s, 8000Hz, 8-bit uPCM mono (~8KB)
  - `examples/truncated.wav`: example.wav truncated to 1000 bytes
  - `examples/zero.wav`: zero-byte file (already exists)
  Run: `ffmpeg -f lavfi -i "sine=f=440:d=5" -ac 1 -ar 8000 -sample_fmt u8 -acodec pcm_u8 examples/long-test.wav`

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Shared infrastructure needed by audio output — ring buffer and signal handling primitives

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [ ] T004 [P] Add crossbeam dependency for SPSC ring buffer in workspace Cargo.toml (`crossbeam = "0.8"`) and crates/around-engine/Cargo.toml
- [ ] T005 [P] Create audio output ring buffer module at crates/around-engine/src/output.rs — crossbeam channel-based PCM f32 sample buffer. Exposes `AudioOutput` struct with `Sender` (decode thread writes samples) and `Receiver` (cpal callback reads samples). Handle buffer underrun by returning silence.
- [ ] T006 Wire output module into crates/around-engine/src/lib.rs (`pub mod output; pub use output::AudioOutput;`)
- [ ] T007 Move `Engine::stop()` and `Engine::running` to `Arc<AtomicBool>` that is shareable between decode thread, signal handler, and IPC server. Currently these are on `Engine` itself; extract to a `SharedState` struct or expose via `Arc<AtomicBool>`.

**Checkpoint**: Ring buffer and shared state ready. Audio output can now be implemented.

---

## Phase 3: User Story 1a — Audio Output Pipeline (Priority: P1) 🎯

**Goal**: `around play examples/long-test.wav` decodes and plays audio through system speakers (or PulseAudio null-sink in CI). The cpal output stream keeps the process alive for the full duration of the WAV, creating a window for IPC commands.

**Why split from IPC**: Without audio output, `Engine::play` decodes a WAV file in milliseconds and exits — there is never a chance to connect IPC commands. cpal's real-time output stream is what keeps the main thread alive and makes IPC testing feasible. The test WAV (T003) is ≥10 seconds to provide a comfortable window.

**Independent Test**: `around play examples/long-test.wav` with PulseAudio null-sink — audio runs for ≥10 seconds. Process exits after playback completes. Ctrl+C stops playback.

**Testing without audio hardware**: Use PulseAudio null-sink module (`pactl load-module module-null-sink`) or snd-aloop. For pure CI without PulseAudio, skip audio-dependent tests with a clear message. All IPC lifecycle tests can use an alternative mode: the IPC server thread itself keeps the engine alive (see US1b).

### Tests for User Story 1a ⚠️

- [ ] T008 [P] [US1a] Write failing unit test for AudioOutput ring buffer at crates/around-engine/tests/output_test.rs — send samples through Sender, verify they arrive correctly at Receiver, verify silence on underrun

### Implementation for User Story 1a

- [ ] T009 [US1a] Implement cpal audio output stream creation in crates/around-engine/src/pipeline.rs — after decoder opens successfully, build a cpal output `Stream` with the decoder's `SampleSpec`. Create the stream with `build_output_stream` and pass the ring buffer `Receiver` into the callback closure. The callback reads f32 samples from the receiver and writes them into cpal's output buffer. On underrun (receiver empty), write silence.
- [ ] T010 [US1a] Wire the decode loop to write decoded PCM samples into the ring buffer `Sender` in crates/around-engine/src/pipeline.rs — replace the current accumulate-only decode loop with a `decoder.read()` → `sender.send()` pattern. The decode loop now runs at real-time speed, blocking when the ring buffer is full (cpal consumes at playback rate).
- [ ] T011 [US1a] Implement cpal stream lifecycle — store the cpal `Stream` in a location where it lives for the duration of playback. On drop, audio stops. Add an error callback that sets `Engine::device_lost` flag on StreamError.
- [ ] T012 [US1a] Generate a test WAV if not done in setup: use `hound` or a simple generator to create examples/long-test.wav (≥10s, 8-bit 8000Hz mono, ~100KB). Alternatively, generate it programmatically in the test itself.

**Checkpoint**: `around play` with a WAV file plays audio at real-time speed through the default audio device. Process stays alive for the file duration. Ctrl+C stops playback instantly.

---

## Phase 4: User Story 1b — IPC Server in cmd_play (Priority: P1)

**Goal**: `around play` starts an IPC server alongside audio output, creating `/tmp/around.sock`. User can run `around status/stop/pause/resume` from a second terminal.

**Independent Test**: (1) `around play examples/long-test.wav` in terminal A. (2) Within 10+ seconds, run `around status` in terminal B → receives `{"status":"ok","state":"playing"}`. (3) `around stop` → terminal A exits, socket file deleted.

**Without audio hardware**: The IPC server runs on a separate tokio thread. Even without audio output, the engine can stay alive until an IPC stop command or the decode loop finishes. For CI tests, use a flag to keep the process alive (see US3 test design).

### Tests for User Story 1b ⚠️

- [ ] T013 [P] [US1b] Write failing integration test for IPC server startup at crates/around-engine/tests/ipc_control.rs — start engine playing a test source, verify `/tmp/around.sock` exists with 0600 permissions, send `status` command and verify `{"status":"ok"}` response

### Implementation for User Story 1b

- [ ] T014 [US1b] Spawn IPC server during `cmd_play` in crates/around-cli/src/main.rs — before starting the decode loop, create a `tokio::runtime::Runtime` with `current_thread` + `enable_io()` on a dedicated OS thread, and `block_on(run_ipc_server("/tmp/around.sock", engine))`. The IPC server thread accepts commands until playback ends.
- [ ] T015 [US1b] After playback completes (file ends, stop, or signal), gracefully shut down the IPC server — drop the tokio runtime (closes listener and socket), verify `/tmp/around.sock` is deleted.
- [ ] T016 [US1b] Wire `Engine::stop()` via shared `Arc<AtomicBool>` running flag — when stop is called (from IPC or signal), the decode loop exits, which signals the main thread to drop the tokio runtime and clean up.

**Checkpoint**: Two-terminal IPC control works. `around status/stop` connects and receives valid responses. Socket created on play, deleted on stop.

---

## Phase 5: User Story 2 — Clean Shutdown on Signal (Priority: P2)

**Goal**: User sends Ctrl+C, SIGHUP, or SIGTERM during playback; the process cleanly stops audio, closes IPC socket, deletes socket file, and exits within 500ms.

**Independent Test**: Start playback, send each signal. Verify: audio stops, `/tmp/around.sock` deleted, process exits cleanly within 500ms.

### Tests for User Story 2 ⚠️

- [ ] T017 [P] [US2] Write failing integration test for signal-triggered cleanup at crates/around-engine/tests/ipc_control.rs — spawn engine, send SIGTERM, verify socket deleted and process exits cleanly

### Implementation for User Story 2

- [ ] T018 [P] [US2] Register SIGHUP and SIGTERM signal handlers via `tokio::signal::unix` in crates/around-cli/src/main.rs — alongside the ctrlc SIGINT handler. All handlers set the same shared `running` flag to false. The tokio signal watchers run inside the IPC server's tokio runtime.
- [ ] T019 [US2] Ensure cleanup sequence runs after signal — after decode loop and IPC server shut down (triggered by `running=false`): drop cpal stream, drop tokio runtime, delete `/tmp/around.sock`, exit cleanly. Cleanup runs on main thread, not in signal handler.

**Checkpoint**: All three signals produce identical complete cleanup. No stale resources.

---

## Phase 6: User Story 3 — Integration Test Coverage (Priority: P2)

**Goal**: Full IPC lifecycle validated by automated tests. Tests run in CI with or without audio hardware.

**Testing strategy without audio hardware**: 
- Tests that only need the IPC server (status, stop, socket lifecycle) can start the engine with an empty/dummy source and rely on the IPC server thread to keep the process alive
- Tests that need audio output use PulseAudio null-sink (`pactl load-module module-null-sink`) or snd-aloop. If neither is available, skip with a clear message
- The long test WAV (≥10s at examples/long-test.wav) provides enough time for IPC commands when audio is available

**Independent Test**: `cargo test --test ipc_control` passes all IPC lifecycle tests (or gracefully skips audio-dependent ones if no device).

### Tests for User Story 3 ⚠️

- [ ] T020 [P] [US3] Write integration test for socket creation and permissions at crates/around-engine/tests/ipc_control.rs — start engine, verify socket exists at `/tmp/around.sock` with 0600 permissions
- [ ] T021 [P] [US3] Write integration test for command round-trip at crates/around-engine/tests/ipc_control.rs — send play, pause, resume, seek, status, stop; verify each returns expected JSON and state transitions
- [ ] T022 [P] [US3] Write integration test for stale socket detection at crates/around-engine/tests/ipc_control.rs — create stale socket file manually, start engine, verify socket replaced with fresh 0600 binding
- [ ] T023 [P] [US3] Write integration test for running-instance rejection at crates/around-engine/tests/ipc_control.rs — start first engine, attempt second, verify error and first socket unchanged
- [ ] T024 [P] [US3] Write integration test for socket cleanup at crates/around-engine/tests/ipc_control.rs — play, stop, verify socket absent
- [ ] T025 [P] [US3] Write integration test for signal-triggered cleanup — play, send SIGTERM, verify socket absent, process exited cleanly

**Checkpoint**: Complete IPC lifecycle coverage — 6 test scenarios pass or gracefully skip based on audio hardware availability.

---

## Phase 7: User Story 4 — Enhancements (Priority: P3)

**Goal**: Static dispatch for WAV hot path, `device_lost` in IPC status, explicit signal handlers, improved KDL error messages.

**Independent Test**: `cargo bench` no regression. IPC status includes `device_lost`. `cargo test` passes.

### Tests for User Story 4 ⚠️

- [ ] T026 [P] [US4] Write test verifying WavDecoder uses concrete type dispatch at crates/around-engine/tests/decoder_contract.rs
- [ ] T027 [P] [US4] Write test verifying IPC status response includes `device_lost` at crates/around-engine/tests/ipc_control.rs

### Implementation for User Story 4

- [ ] T028 [P] [US4] Restore static dispatch for WAV decoder in crates/around-engine/src/pipeline.rs — when selected decoder is "wav-builtin", use `around_codec_wav::WavDecoder::open(source)` directly (concrete type, no vtbl). Keep `try_open_decoder` for fallback/extension scenarios only.
- [ ] T029 [P] [US4] Add `device_lost: bool` field to IPC status response in crates/around-engine/src/ipc.rs
- [ ] T030 [US4] Improve KDL parse error messages in crates/around-engine/src/config.rs — include file path and line/column in parse error output using kdl-rs error span info
- [ ] T031 [US4] Ensure SIGHUP/SIGTERM handlers are explicitly registered via tokio signal in crates/around-cli/src/main.rs — move from ctrlc-only to explicit tokio watchers for all three signals

**Checkpoint**: Performance parity restored, IPC enhanced, signals explicit. All tests pass.

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Final validation, formatting, and cleanup

- [ ] T032 [P] Run `cargo fmt` and `cargo clippy` across the workspace, fix all warnings
- [ ] T033 Run `cargo test --workspace` — all tests pass (audio-dependent tests gracefully skipped if no device)
- [ ] T034 Run `cargo build --release` — binary stripped size <10MB
- [ ] T035 Update specs/002-mvp-audio-output/tasks.md checkboxes to mark all completed tasks as [X]

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — verify workspace + generate test WAV
- **Foundational (Phase 2)**: Depends on Setup — BLOCKS all stories
- **US1a — Audio Output (Phase 3)**: Depends on Foundational — NO dependency on IPC
- **US1b — IPC Server (Phase 4)**: Depends on US1a **critically** — without audio output keeping the process alive, IPC server has no window to receive commands
- **US2 — Signal Cleanup (Phase 5)**: Depends on US1a + US1b
- **US3 — Integration Tests (Phase 6)**: Depends on US1a + US1b + US2
- **US4 — Enhancements (Phase 7)**: Depends on US1a only — can run in parallel with US1b/US2/US3
- **Polish (Phase 8)**: Depends on all desired stories complete

### Critical Implementation Order

```
1. Audio output (cpal stream, ring buffer wiring) ← MUST BE FIRST
   ↓ process now stays alive for WAV duration (≥10s with long-test.wav)
2. IPC server (tokio runtime in background thread) ← now has command window
   ↓
3. Signal handlers + cleanup sequence
   ↓
4. Integration tests (skip audio-dependent if no hardware)
```

### Parallel Opportunities

- **Phase 2**: T004 (crossbeam) and T005 (output module) are [P]
- **Phase 7**: T028 (static dispatch), T029 (device_lost), T031 (signal handlers) are [P]
- **Phase 6**: T020–T025 all [P] — different test functions in same file

---

## Implementation Strategy

### MVP First (US1a + US1b Only)

1. Phase 1: Setup — verify workspace + generate long-test.wav
2. Phase 2: Foundational — ring buffer + shared state
3. Phase 3: US1a — audio output via cpal, process stays alive for ≥10s
4. Phase 4: US1b — IPC server in cmd_play, socket created
5. **STOP and VALIDATE**: Two-terminal test works
6. This is the MVP — playable audio with IPC control

### Testing Without Audio Hardware (CI)

- **PulseAudio null-sink**: `pactl load-module module-null-sink` creates a virtual output device. cpal can target it.
- **snd-aloop**: Linux kernel module (`modprobe snd-aloop`) creates virtual loopback ALSA device.
- **No audio at all**: IPC-only tests keep the engine alive via the running IPC server thread. Audio-dependent tests skip with a clear "no audio device" message.
- The long test WAV (T003) at ≥10 seconds gives ample time for commands regardless.

---

## Notes

- [P] tasks = different files, no dependencies — run in parallel
- [US1a]/[US1b]/[US2]/[US3]/[US4] labels map tasks to specific user stories
- Constitution mandates test-first; all test tasks MUST fail before implementation
- **US1a MUST precede US1b** — without audio output keeping the process alive for ≥10 seconds, IPC commands have no window to arrive; the process decodes in <0.1s and exits
- cpal's `build_output_stream` callback runs on a real-time audio thread — channel reads must never block; use `try_recv` with silence fallback on underrun
- The IPC server's tokio runtime is also used for signal watching (SIGHUP/SIGTERM via tokio::signal::unix)
