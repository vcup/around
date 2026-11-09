# Feature Specification: MVP Audio Output Pipeline & Critical Fixes

**Feature Branch**: `002-mvp-audio-output`

**Created**: 2026-05-28

**Status**: Draft

**Input**: Based on issues identified in the post-implementation analysis (hot-path dyn dispatch, incomplete signal cleanup, missing device status in IPC), plus the current MVP having no actual audio output and the IPC server never being started during `play` — determine what work can be done in the current phase.

**Scope note**: This historical feature specification retains its original
acceptance criteria. Later ADRs supersede earlier architecture choices; current
implementation notes below are not evidence that every original latency,
audio-device, or signal-handling criterion has been verified.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Hear Audio Output with IPC Control (Priority: P1)

A user runs `around play examples/long-test.wav`, hears the WAV file through the
system speakers, and controls playback from another terminal with
`pause`/`resume`/`stop`/`status`. The IPC endpoint is created when playback
starts and cleaned up when the engine exits.

**Why this priority**: Audio output is the product's core value. IPC must start
with playback so control commands can reach the running engine.

**Independent Test**: Start playback, query `status`, verify the response and
endpoint permissions, then issue `stop` and verify process and endpoint cleanup.

**Acceptance Scenarios**:

1. **Given** a WAV file exists at a known path, **When** the user runs `around play <path>`, **Then** audio begins playback within 2 seconds AND the IPC socket `/tmp/around.sock` is created, allowing another terminal to connect and send commands before the file finishes.
2. **Given** playback is in progress, **When** the user runs `around status` from another terminal, **Then** a valid JSON response with `"status": "ok"` and `"state": "playing"` is returned.
3. **Given** playback is in progress, **When** the user runs `around stop` from another terminal, **Then** playback stops, the process exits within 500ms, and `/tmp/around.sock` is deleted.
4. **Given** playback is in progress, **When** the user runs `around pause` then `around resume`, **Then** playback pauses and resumes correctly.
5. **Given** a file path that does not exist, **When** the user runs `around play <path>`, **Then** a clear error is displayed and no socket file is created.

**Endpoint resolution (current)**: The default Unix endpoint is selected from unique,
ordered candidates: a non-empty `$XDG_RUNTIME_DIR/around/around.sock`, the
standard temporary directory's `around.sock`, then `/tmp/around.sock`. A
configured custom path is the sole candidate. The `/tmp/around.sock` references
in the historical acceptance criteria above denote the final fallback; tests
assert against the path actually bound.

---

### User Story 2 - Clean Shutdown on Signal (Priority: P2)

A user interrupts playback via Ctrl+C, SIGHUP, or SIGTERM and the process cleans up all resources (IPC socket, audio device) without leaving stale files.

**Why this priority**: FR-004 of the core spec requires all three signals to execute the same full cleanup sequence. At feature inception, the implementation only stopped the decode loop; it did not close the IPC socket, release the audio device, or delete the socket file. Incomplete cleanup can cause startup errors on the next run or resource leaks.

**Independent Test**: Start playback of a 5-second WAV, then send SIGINT, SIGHUP, and SIGTERM signals respectively. Verify each time the process exits cleanly, `/tmp/around.sock` is deleted, and no audio resources remain.

**Acceptance Scenarios**:

1. **Given** playback is in progress, **When** the user sends SIGINT (Ctrl+C), **Then** the engine stops playback, closes the IPC socket, releases the audio device, deletes the socket file, and exits within 500ms.
2. **Given** playback is in progress, **When** the user sends SIGTERM, **Then** the same full cleanup sequence is executed.
3. **Given** playback is in progress, **When** the user sends SIGHUP, **Then** the same full cleanup sequence is executed.

---

### User Story 3 - Integration Test Coverage for End-to-End Flow (Priority: P2)

A developer can run the full test suite and verify that the IPC lifecycle (socket creation, command handling, socket cleanup), audio pipeline, and control commands work correctly from end to end. Tests gracefully handle environments without audio hardware.

**Why this priority**: At feature inception, the test suite lacked coverage for the IPC-server-during-play lifecycle — the fundamental bug that `cmd_play` never started `run_ipc_server` was not caught by tests. Integration tests must validate the complete flow to prevent regressions.

**Independent Test**: Run the IPC suites (`ipc_unix_socket` in `around-engine` and `ipc_native` in `around-cli`) — all tests pass (audio-dependent tests gracefully skip if no audio device is available). Tests use `examples/long-test.wav` (5s) or a dummy infinite source to keep the engine alive during IPC command round-trips.

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

1. **Given** the source file is WAV format, **When** the engine selects a decoder and begins playback, **Then** the WAV decoder's `read()` method is called through static dispatch, not through a `dyn Trait` vtable.
2. **Given** a fallback scenario exists, **When** the WAV decoder `open()` fails, **Then** the engine falls back to other registered decoders using `Box<dyn Decoder>` (acceptable because fallback is not on the hot path).

---

### Edge Cases

- What happens when the audio device is unavailable or already in use? Return a
  descriptive error; do not panic or leave a stale IPC endpoint.
- What happens when the audio device disconnects during playback? Detect
  `StreamError`, set `device_lost`, and auto-pause; recovery is configurable.
- How does CI behave without audio hardware? Degrade with a clear error, or use
  a project-local virtual device where available; IPC-only tests remain runnable.
- What happens when a command arrives during initialization? Return the current
  state, such as `"buffering"`.
- What happens if the IPC endpoint is deleted during playback? Continue playback
  and log the next connection failure.
- What happens when the endpoint path exists as a regular file or symlink?
  Preserve it and fail the bind with `AddrInUse`; no TCP fallback occurs. Only a
  Unix socket that refuses a probe connection is treated as stale, and a live
  endpoint also yields `AddrInUse`.
- Signal handlers set the shared atomic flag only; the main loop performs cleanup.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `around play` MUST start the IPC server before audio output and
  create the platform-appropriate endpoint with its required permissions.
  During binding, a live native endpoint or a non-socket path MUST fail with
  `AddrInUse` and MUST NOT trigger the TCP fallback.
- **FR-002**: The engine MUST create an output stream for the negotiated
  `OutputSession` format and feed it native-format bytes produced by FilterChain.
- **FR-003**: Stop signals and the IPC `stop` command MUST silence audio and
  release the output stream within 500ms.
- **FR-004**: SIGINT, SIGHUP, and SIGTERM MUST share one cleanup sequence:
  stop playback, close IPC, release audio, remove endpoint, and exit cleanly.
  Signal handlers MUST only set a shared atomic flag.
- **FR-005**: When playback ends, the IPC endpoint MUST be removed during engine
  cleanup.
- **FR-006**: The built-in WAV decoder MUST use static dispatch in the hot loop;
  trait objects are limited to extension and fallback paths.
- **FR-007**: IPC status MUST include `device_lost: bool`.
- **FR-008**: KDL parse errors MUST include file path and line number.
- **FR-009**: SIGHUP and SIGTERM handlers MUST be registered explicitly.
- **FR-010**: Foreground `around play` MUST keep audio and IPC alive together.
  Stream, IPC, or socket failures MUST trigger graceful stop.
- **FR-011**: Output stream creation failures MUST return a descriptive error
  without panic or a stale IPC endpoint.
- **FR-012**: Integration tests MUST cover endpoint creation, command
  serialization/response, stale and live endpoint detection, and cleanup after
  stop or signal. Audio-dependent tests MUST handle missing hardware.
- **FR-013**: CLI commands MUST exit with status `1` and print
  `error: <message>` to stderr when execution fails, whether the endpoint cannot
  be reached or the server returns a non-success `IpcResponse`. A server error
  response MUST NOT be reported as success.
- **FR-014**: Unless `--remote` is supplied, the CLI MUST try the shared native
  endpoint candidates first and then the `<temp>/around.port` TCP fallback.
  Trying another Unix candidate retries connection establishment only, not a
  command already written to a Unix connection.

### Key Entities

- **Audio Output Stream**: A handle to the active audio-device connection.
  Created for playback and dropped when playback stops.
- **Device Status**: `device_lost` reflects audio-device connectivity, updated
  by the stream error callback and exposed through IPC status.
- **IPC Server**: The tokio listener accepting control commands. It starts with
  `cmd_play` and terminates during engine cleanup.
- **Audio Output Ring Buffer**: A bounded SPSC ring owned by the output binding.
  The decode owner queues native-format bytes and applies backpressure when the
  ring is full; the cpal callback copies bytes and emits encoding-correct silence
  on underrun (ADR-0006).
- **Shared Playback State**: Per ADR-0005, each stream has an atomic
  `StreamState` (status, position, device loss) read by IPC handlers through
  `Engine`. The pre-ADR-0005 single `Arc<Mutex<PlaybackState>>` is superseded.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A user can run `around play examples/long-test.wav` and hear audio output through the system speakers within 2 seconds.
- **SC-002**: A user can run `around status` from another terminal during playback and receive a valid JSON response within 1 second.
- **SC-003**: After Ctrl+C or SIGTERM, the process exits cleanly within 500ms and no stale `/tmp/around.sock` file remains.
- **SC-004**: Pipeline latency benchmark shows no regression from the static-to-dyn dispatch fix (less than 1% difference from the pure static dispatch baseline).
- **SC-005**: `cargo test` passes on Linux both with and without an audio device present (audio-dependent tests gracefully skip with a clear message when no device is available).
- **SC-006**: The integration test for IPC lifecycle (`ipc_control`) passes, confirming socket creation, command handling, stale socket detection, running-instance rejection, and socket cleanup.

**Regression coverage**: `ipc_unix_socket` exercises permissions, command
round-trips, stale socket recovery, duplicate rejection with and without the
pre-check, preservation of ordinary files and symlinks, and custom-path cleanup.
The CLI's `ipc_native` test exercises directory-preparation and bind-time
fallback, server error exit codes, `stop` keeping IPC reachable, and shutdown
cleanup. These tests do not claim audio timing or arbitrary signal coverage.

## Assumptions

- Linux with PulseAudio or ALSA is the primary audio output target. macOS and Windows backends remain deferred — no changes are made to those platform stubs.
- The existing `cpal` dependency is sufficient; no new audio libraries are required.
- WAV is the only built-in decoder; the static dispatch optimization applies only to built-in decoders, not extensions.
- The `output_auto_reconnect` config field (already added in T062) is wired to control device-loss recovery behavior.
- At feature inception, `run_ipc_server` was implemented but `cmd_play` did not invoke it — that was the core bug addressed by this feature.
  - A 5-second low-bitrate WAV (`examples/long-test.wav`) is generated during setup for IPC testing. It uses 8000Hz/8-bit/mono to keep file size under ~40KB.
- Environments without audio hardware use PulseAudio null-sink, snd-aloop, or skip audio-dependent tests gracefully.
