# Feature Specification: Cross-Platform MVP Foundation

**Feature Branch**: `002-mvp-audio-output`

**Created**: 2026-05-31

**Status**: Draft

**Input**: Based on investigation from the codec-system-v2 redesign and cross-platform CI analysis. The MVP has evolved beyond the original `002-mvp-audio-output` scope: the codec architecture was fully redesigned (Decoder → Codec with fn-pointer vtable), but the platform abstraction layer and CI pipeline were left incomplete. This specification captures the consolidation work needed to make the MVP compile, test, and pass CI on Linux, macOS, and Windows per Constitution Principle IV.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Engine Compiles on All Target Platforms (Priority: P1)

A developer clones the repository on Linux, macOS, or Windows, runs `cargo build --workspace`, and the engine crate (`around-engine`) compiles without errors on all three platforms. The CLI binary is available on Linux and macOS; on Windows it is explicitly not yet available (gated behind `#[cfg(unix)]`).

**Why this priority**: Constitution IV mandates "compile and run on Linux, macOS, and Windows." The current HEAD has un-gated Unix-only code (`UnixStream`, `UnixListener`, POSIX signal handling) in `ipc.rs`, `lib.rs`, and `main.rs` that prevents Windows compilation. This is a blocker for CI and for any Windows contributor.

**Independent Test**: GitHub Actions CI runs `cargo build --workspace` on Linux, macOS, and Windows runners — all three pass with zero errors. Locally, `rustup target add x86_64-pc-windows-msvc && cargo check --workspace --target x86_64-pc-windows-msvc` or equivalent cross-check passes.

**Acceptance Scenarios**:

1. **Given** the repository is checked out on Linux, **When** `cargo build --workspace` runs, **Then** compilation succeeds with zero errors and zero warnings.
2. **Given** the repository is checked out on macOS, **When** `cargo build --workspace` runs, **Then** compilation succeeds with zero errors and zero warnings.
3. **Given** the repository is checked out on Windows, **When** `cargo build --workspace` runs, **Then** the `around-engine` and `around-core` crates compile successfully; the `around-cli` crate is skipped (gated behind `#[cfg(unix)]`).

---

### User Story 2 - IPC Types Cleanly Separated from Transport (Priority: P1)

The IPC type definitions (`PlaybackState`, `IpcCommand`, `IpcResponse`) are defined in a platform-independent module (`ipc_types.rs`) that always compiles. The Unix socket IPC server lives in a `#[cfg(unix)]`-gated module (`ipc.rs`). No dead code modules remain in the crate tree.

**Why this priority**: The current code has `ipc_types.rs` missing, `ipc_codec.rs` with an unused `JsonLineCodec` struct (dead code causing CI failure), and `ipc_transport.rs` similarly unwired. The module structure is incoherent and blocks CI. This split is the architectural foundation for future Windows IPC (named pipes).

**Independent Test**: `cargo clippy --workspace -- -D warnings` passes with `-D dead-code` enabled. No `dead_code` warnings appear. The `ipc_types` module compiles on all platforms. `ipc.rs` compiles only on Unix.

**Acceptance Scenarios**:

1. **Given** the engine crate is compiled, **When** `PlaybackState`, `IpcCommand`, `IpcResponse` are imported, **Then** they resolve from `around_engine::ipc_types` regardless of target platform.
2. **Given** the engine crate is compiled on Unix, **When** `run_ipc_server` is imported, **Then** it resolves from `around_engine::ipc` (Unix-gated).
3. **Given** the workspace is linted with `cargo clippy -- -D warnings`, **When** dead code detection runs, **Then** no `dead_code` or `never_constructed` warnings are emitted.

---

### User Story 3 - Full Test Suite Passes on All Platforms (Priority: P2)

All tests that do not depend on Unix IPC infrastructure pass on Linux, macOS, and Windows. Unix-specific IPC tests are gated behind `#[cfg(unix)]` and skip gracefully on non-Unix platforms. The total cross-platform test count is verified.

**Why this priority**: Confidence in cross-platform correctness requires automated test coverage on all target OS. Currently only Linux is tested. The IPC tests (33) are already gated — they just need verification that they skip correctly on non-Unix.

**Independent Test**: `cargo test --workspace` on Linux passes 88 tests (55 cross-platform + 33 IPC). On Windows/macOS, 55 tests pass and 33 skip. CI on all three platforms shows green test results.

**Acceptance Scenarios**:

1. **Given** tests run on Linux, **When** `cargo test --workspace` executes, **Then** 88 tests pass (55 cross-platform + 33 IPC), zero failures.
2. **Given** tests run on Windows or macOS, **When** `cargo test --workspace` executes, **Then** 55 cross-platform tests pass, 33 IPC tests are skipped (not failed), zero failures.
3. **Given** the CI workflow runs on pull request, **When** the test step executes on all three platforms, **Then** all three jobs show green (tests pass or skip appropriately).

---

### User Story 4 - Dead Code and Unused Modules Removed (Priority: P2)

The repository contains no unused Rust source files, no dead code structures, and no module declarations pointing to non-existent or unused modules. Every `pub mod` declaration in `lib.rs` maps to a file that is actually used by the crate or its consumers.

**Why this priority**: Dead code adds compilation cost, confuses contributors, and triggers `-D warnings` failures in CI. The current `ipc_codec.rs` (JsonLineCodec never constructed) and `ipc_transport.rs` (never wired into any connection handler) are the specific offenders.

**Independent Test**: `cargo clippy --workspace -- -D warnings` passes. Grep for `JsonLineCodec` finds zero references outside deleted file. `ipc_transport.rs` file does not exist.

**Acceptance Scenarios**:

1. **Given** the repository, **When** `crates/around-engine/src/ipc_transport.rs` is checked, **Then** the file does not exist.
2. **Given** the repository, **When** `crates/around-engine/src/ipc_codec.rs` is checked, **Then** the file does not exist.
3. **Given** the workspace is built with `-D warnings`, **When** the compiler checks for dead code, **Then** no dead code warnings are emitted.

---

### Edge Cases

- What happens on Windows when a test imports `PlaybackState`? → It compiles and works because `PlaybackState` is in the always-compiled `ipc_types` module.
- What happens if CI runner has no audio device? → Audio-dependent tests skip gracefully (existing behavior via cpal device detection).
- What happens when a developer adds a new Unix-only API without `#[cfg]` gating? → CI catches it on Windows/macOS builds (the whole point of cross-platform CI).
- What happens if `libloading` behaves differently on Windows? → Already handled — `DLL_EXTENSION` constant in `extensions.rs` selects `.dll` vs `.so`/`.dylib`.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The `around-engine` crate MUST compile on Linux, macOS, and Windows targets without errors.
- **FR-002**: The `ipc_types` module MUST be always-compiled (no `#[cfg]` gating) and contain `PlaybackState`, `IpcCommand`, and `IpcResponse` type definitions.
- **FR-003**: The `ipc` module (Unix socket server) MUST be gated behind `#[cfg(unix)]` and MUST NOT be compiled on Windows.
- **FR-004**: The `around-cli` crate MUST be gated behind `#[cfg(unix)]` — it MUST NOT be compiled on Windows.
- **FR-005**: The files `ipc_codec.rs` and `ipc_transport.rs` MUST NOT exist in the repository.
- **FR-006**: The `lib.rs` module declarations MUST be consistent with actual files — no orphaned `pub mod` declarations.
- **FR-007**: `PlaybackState`, `IpcCommand`, and `IpcResponse` MUST be re-exported from the crate root (`around_engine::PlaybackState`, etc.) via `ipc_types`, not `ipc`.
- **FR-008**: `run_ipc_server` MUST be re-exported from the crate root ONLY on Unix targets (`#[cfg(unix)]`).
- **FR-009**: The CI workflow MUST include Linux, macOS, and Windows build+test jobs.
- **FR-010**: All 55 cross-platform tests MUST pass on all three platforms.
- **FR-011**: All 33 Unix IPC tests MUST pass on Linux and be skipped (not failed) on Windows and macOS.
- **FR-012**: `cargo clippy --workspace -- -D warnings` MUST pass with zero warnings on all platforms.

### Key Entities

- **PlaybackState**: Shared state struct (position, duration, playing/paused/stopped, device_lost, seekable). Always compiled — used by pipeline and IPC both.
- **IpcCommand**: Enum of commands accepted by the IPC server (Play, Pause, Resume, Seek, Stop, Status, LoadDecoder, ListDecoders, Cleanup). Always compiled so CLI can construct commands.
- **IpcResponse**: Struct for IPC responses (status field, state, position, error message). Always compiled.
- **IpcServer**: The Unix socket listener + per-connection handler. Unix-only. Gated behind `#[cfg(unix)]`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: `cargo build --workspace` exits with code 0 on Linux, macOS, and Windows (verified by CI).
- **SC-002**: `cargo test --workspace` passes 88 tests on Linux, 55 on Windows, 55 on macOS (verified by CI).
- **SC-003**: `cargo clippy --workspace -- -D warnings` exits with code 0 on all platforms (verified by CI).
- **SC-004**: Zero dead-code warnings in the engine crate, verified by clippy.
- **SC-005**: The CI pipeline runs all three platform jobs on every pull request to `master` and reports results within 10 minutes.
- **SC-006**: A developer on Windows can `cargo test -p around-engine` and see 55 tests pass with meaningful output (no cryptic compilation errors from un-gated Unix code).

## Assumptions

- `cpal` handles cross-platform audio output (ALSA/PulseAudio on Linux, CoreAudio on macOS, WASAPI on Windows) — no engine changes needed for audio.
- `libloading` handles cross-platform dynamic library loading — `DLL_EXTENSION` constant already selects correct suffix.
- The CLI (`around-cli`) is a developer convenience tool for Linux/macOS. Windows CLI support (named pipes) is deferred to a future feature.
- The existing 88 tests on Linux are correct and must continue passing unchanged.
- GitHub Actions provides `ubuntu-latest`, `macos-latest`, and `windows-latest` runners with Rust toolchain support.
- The `JsonLineCodec` abstraction was premature — JSON parsing inline in `ipc.rs` suffices for the single-codec case. If a protobuf codec is added later, the abstraction can be reintroduced when actually needed.
