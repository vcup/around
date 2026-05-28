# Feature Specification: MVP Audio Output Pipeline & Critical Fixes

**Feature Branch**: `002-mvp-audio-output`

**Created**: 2026-05-28

**Status**: Draft

**Input**: Based on issues identified in the post-implementation analysis (hot-path dyn dispatch, incomplete signal cleanup, missing device status in IPC), plus the current MVP having no actual audio output and the IPC server never being started during `play` — determine what work can be done in the current phase.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Hear Audio Output with IPC Control (Priority: P1)

A user runs `around play examples/long-test.wav`, hears the WAV file playback through their system speakers, and can control playback via separate `around pause/resume/stop/status` commands in another terminal. The IPC socket `/tmp/around.sock` is created when playback starts and cleaned up when it ends.

**Why this priority**: Audio output is the core value proposition of a player — without sound, the decode pipeline is just CPU computation. Additionally, the IPC server must be started alongside playback so that transport control commands can connect to the running engine. In the current implementation `cmd_play` never starts `run_ipc_server`, so all IPC commands fail with "connection refused". This bug must be fixed to make the complete end-to-end flow work. A 5-second low-bitrate WAV (`long-test.wav`, ~40KB) provides enough time for IPC commands to arrive before the file finishes.

**Independent Test**: Two-terminal test: (1) Start `around play examples/long-test.wav` (5 seconds, ~40KB) in terminal A. (2) In terminal B, run `around status` — verify response includes `"status": "ok"` and `"state": "playing"`. (3) Verify `/tmp/around.sock` exists with 0600 permissions. (4) Run `around stop` — verify terminal A exits within 500ms and `/tmp/around.sock` is removed.

**Acceptance Scenarios**:

1. **Given** a WAV file exists at a known path, **When** the user runs `around play <path>`, **Then** audio begins playback within 2 seconds AND the IPC socket `/tmp/around.sock` is created, allowing another terminal to connect and send commands before the file finishes.
2. **Given** playback is in progress, **When** the user runs `around status` from another terminal, **Then** a valid JSON response with `"status": "ok"` and `"state": "playing"` is returned.
3. **Given** playback is in progress, **When** the user runs `around stop` from another terminal, **Then** playback stops, the process exits within 500ms, and `/tmp/around.sock` is deleted.
4. **Given** playback is in progress, **When** the user runs `around pause` then `around resume`, **Then** playback pauses and resumes correctly.
5. **Given** a file path that does not exist, **When** the user runs `around play <path>`, **Then** a clear error is displayed and no socket file is created.

---

### User Story 2 - Clean Shutdown on Signal (Priority: P2)

A user interrupts playback via Ctrl+C, SIGHUP, or SIGTERM and the process cleans up all resources (IPC socket, audio device) without leaving stale files.

**Why this priority**: FR-004 of the core spec requires all three signals to execute the same full cleanup sequence. The current implementation only stops the decode loop; it does not close the IPC socket, release the audio device, or delete the socket file. Incomplete cleanup can cause startup errors on the next run or resource leaks.

**Independent Test**: Start playback of a 5-second WAV, then send SIGINT, SIGHUP, and SIGTERM signals respectively. Verify each time the process exits cleanly, `/tmp/around.sock` is deleted, and no audio resources remain.

**Acceptance Scenarios**:

1. **Given** playback is in progress, **When** the user sends SIGINT (Ctrl+C), **Then** the engine stops playback, closes the IPC socket, releases the audio device, deletes the socket file, and exits within 500ms.
2. **Given** playback is in progress, **When** the user sends SIGTERM, **Then** the same full cleanup sequence is executed.
3. **Given** playback is in progress, **When** the user sends SIGHUP, **Then** the same full cleanup sequence is executed.

---

### User Story 3 - Integration Test Coverage for End-to-End Flow (Priority: P2)

A developer can run the full test suite and verify that the IPC lifecycle (socket creation, command handling, socket cleanup), audio pipeline, and control commands work correctly from end to end. Tests gracefully handle environments without audio hardware.

**Why this priority**: The current test suite lacks coverage for the IPC-server-during-play lifecycle — the fundamental bug that `cmd_play` never starts `run_ipc_server` was not caught by tests. Integration tests must validate the complete flow to prevent regressions.

**Independent Test**: Run `cargo test --test ipc_control` — all tests pass (audio-dependent tests gracefully skip if no audio device is available). Tests use `examples/long-test.wav` (5s) or a dummy infinite source to keep the engine alive during IPC command round-trips.

**Acceptance Scenarios**:

1. **Given** the engine starts and plays a file, **When** the test inspects the filesystem, **Then** the IPC socket file exists at the expected path with 0600 permissions.
2. **Given** the IPC server is running with active playback, **When** the test sends play, pause, resume, seek, status, and stop commands in sequence, **Then** each command returns the expected response and state transitions correctly.
3. **Given** playback ends (file completes or engine stops), **When** the test checks the socket path, **Then** the socket file has been removed.
4. **Given** the engine starts on a path where a stale socket file exists, **When** the engine binds, **Then** the stale socket is detected (connection refused), deleted, and a new socket is created.
5. **Given** a second engine instance is started while the first is running, **When** it attempts to bind the same socket path, **Then** it detects the live socket, returns an error, and exits without overwriting the socket.

---

### User Story 4 - Zero-Overhead Hot Path Decoding (Priority: P3)

The built-in WAV decoder is called through static dispatch with no vtable overhead. The fallback mechanism uses `Box<dyn Decoder>` only for extension-loaded decoders, not for the built-in path.

**Why this priority**: Constitution III requires "no `dyn Trait` in hot paths." The `try_open_decoder()` function introduced by T061 downgraded the built-in WAV decoder call path from static dispatch to trait-object dispatch, adding per-call indirection in the decode loop. This violates the performance design principle.

**Independent Test**: Benchmark pipeline latency via `cargo bench` — verify no regression from the static-to-dyn dispatch fix. Compare decode throughput with concrete `WavDecoder` vs. `Box<dyn Decoder>`.

**Acceptance Scenarios**:

1. **Given** the source file is WAV format, **When** the engine selects a decoder and begins playback, **Then** the WAV decoder's `read()` method is called through static dispatch, not through `dyn Trait` vtable.
2. **Given** a fallback scenario exists, **When** the WAV decoder `open()` fails, **Then** the engine falls back to other registered decoders using `Box<dyn Decoder>` (acceptable since fallback is not on the hot path).

---

### Edge Cases

- What happens when the audio device is unavailable or already in use? → Return a descriptive device error; do not panic and do not leave a stale socket file.
- What happens when the audio device is disconnected during playback (e.g., USB audio unplugged)? → Engine detects cpal StreamError, auto-pauses, reports device loss via IPC status.
- How does the engine behave in environments with no audio hardware (e.g., CI containers)? → cpal may fail to create an output stream. Degrade gracefully with an error message instead of panicking. Use PulseAudio null-sink (`module-null-sink`) or snd-aloop for virtual audio when available.
- What happens when a control command arrives before playback has fully initialized (socket created but engine not ready)? → IPC server accepts the connection and returns the current state (e.g., `"state": "buffering"`).
- What happens if the socket file is deleted externally while playback is active? → Next control command connection fails; engine continues playing but reports socket error on next error check.
- Is it safe to perform complex operations inside the signal handler? → Signal handler only sets an atomic flag; the main loop detects the flag and performs the actual cleanup.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST start the IPC server (`run_ipc_server`) when `around play` is invoked, creating the Unix domain socket at `/tmp/around.sock` with 0600 permissions before audio output begins.
- **FR-002**: System MUST create an audio output stream with the correct sample rate and channel count from the decoded format, and feed decoded f32 PCM samples into the output callback.
- **FR-003**: System MUST stop audio output and release the output stream within 500ms of receiving a stop signal (SIGINT/SIGHUP/SIGTERM) or an IPC stop command.
- **FR-004**: The process MUST execute the complete cleanup sequence in response to SIGINT, SIGHUP, or SIGTERM: stop playback, close the IPC socket, release the audio device, delete the socket file, and exit cleanly. The signal handler MUST only set a shared atomic flag; the actual cleanup runs on the main thread (async-signal-safe).
- **FR-005**: When playback ends (file completes or engine stops), the IPC socket file MUST be deleted from the filesystem.
- **FR-006**: The built-in WAV decoder MUST use static dispatch (concrete `WavDecoder` type) in the hot decode loop. `Box<dyn Decoder>` is reserved for extension-loaded decoders and fallback paths only.
- **FR-007**: The IPC status response MUST include a `device_lost: bool` field indicating whether the audio output device is connected.
- **FR-008**: KDL parse error messages MUST include the file path and line number of the parse failure location.
- **FR-009**: Signal handlers for SIGHUP and SIGTERM MUST be registered explicitly (not relying on the ctrlc crate's platform-specific behavior for coverage).
- **FR-010**: The `around play` foreground process MUST keep both the audio output stream and the IPC server alive for the duration of playback. Stream drops, IPC errors, or socket failures MUST trigger a graceful stop.
- **FR-011**: When audio output stream creation fails (no audio device, device busy, permissions), the engine MUST return a descriptive error without panicking and MUST NOT leave a stale socket file.
- **FR-012**: Integration tests MUST cover the full IPC lifecycle: socket creation during play, command serialization and response, stale socket detection and cleanup, running-instance detection, and socket cleanup after stop or signal. Tests MUST gracefully handle environments without audio hardware.

### Key Entities

- **Audio Output Stream**: A handle to the active audio device connection. Its lifecycle is tied to playback — created on play, dropped on stop.
- **Device Status**: A boolean indicator (`device_lost`) reflecting audio device connectivity. Updated by the stream error callback and queried via IPC status.
- **IPC Server**: The tokio-based Unix socket listener that accepts CLI commands. Its lifecycle is started by `cmd_play`, bound at `/tmp/around.sock`, and terminated when playback ends.
- **Audio Output Ring Buffer**: A bounded SPSC channel (`AudioOutput`) that decouples the decode thread (producer) from the cpal callback (consumer). Enforces backpressure via blocking `send()` when full; callback uses non-blocking `try_recv()` with silence on underrun.
- **Shared Playback State**: An `Arc<Mutex<PlaybackState>>` shared between the decode thread (updates `position_ms`, `state`) and IPC handlers (reads for `status`, mutates for `pause`/`resume`/`seek`). Also carries `device_lost`, `duration_ms`, and track metadata.
## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A user can run `around play examples/long-test.wav` and hear audio output through the system speakers within 2 seconds.
- **SC-002**: A user can run `around status` from another terminal during playback and receive a valid JSON response within 1 second.
- **SC-003**: After Ctrl+C or SIGTERM, the process exits cleanly within 500ms and no stale `/tmp/around.sock` file remains.
- **SC-004**: Pipeline latency benchmark shows no regression from the static-to-dyn dispatch fix (less than 1% difference from the pure static dispatch baseline).
- **SC-005**: `cargo test` passes on Linux both with and without an audio device present (audio-dependent tests gracefully skip with a clear message when no device is available).
- **SC-006**: The integration test for IPC lifecycle (`ipc_control`) passes, confirming socket creation, command handling, stale socket detection, running-instance rejection, and socket cleanup.

## Assumptions

- Linux with PulseAudio or ALSA is the primary audio output target. macOS and Windows backends remain deferred — no changes are made to those platform stubs.
- The existing `cpal` dependency is sufficient; no new audio libraries are required.
- WAV is the only built-in decoder; the static dispatch optimization applies only to built-in decoders, not extensions.
- The `output_auto_reconnect` config field (already added in T062) is wired to control device-loss recovery behavior.
- The IPC server currently has `run_ipc_server` implemented but `cmd_play` does not invoke it — this is the core bug to fix.
  - A 5-second low-bitrate WAV (`examples/long-test.wav`) is generated during setup for IPC testing. It uses 8000Hz/8-bit/mono to keep file size under ~40KB.
- Environments without audio hardware use PulseAudio null-sink, snd-aloop, or skip audio-dependent tests gracefully.
