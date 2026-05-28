# Implementation Plan: MVP Audio Output Pipeline & Critical Fixes

**Branch**: `002-mvp-audio-output` | **Date**: 2026-05-28 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `specs/002-mvp-audio-output/spec.md`

## Summary

Complete the MVP audio playback pipeline by wiring up cpal audio output and fixing the critical IPC server lifecycle bug (cmd_play never starts run_ipc_server). Also address analysis findings: restore static dispatch for built-in WAV decoder, complete signal cleanup sequence, report device status via IPC, and add integration tests for the full IPC lifecycle.

## Technical Context

**Language/Version**: Rust (stable, edition 2021)

**Primary Dependencies**: 
- `cpal` — audio output stream (already in Cargo.toml but not wired into pipeline)
- `tokio` — async runtime for IPC server (already wired)
- `tracing` — structured logging (already wired)

**Storage**: N/A (no persistent state)

**Testing**: `cargo test` (existing contract + integration tests), plus expanded IPC lifecycle tests

**Target Platform**: Linux (PulseAudio/ALSA); macOS/Windows backends remain deferred

**Project Type**: Rust workspace — feature completes existing 001-core-playback-engine crates

**Performance Goals**: Restore static dispatch benchmark parity; <500ms shutdown on signal; <2s audio start

**Constraints** (see also Testing Discipline below): 
- cpal output stream must be created on the OS audio thread (callback-based)
- Signal handler must remain async-signal-safe (atomic flags only)
- IPC socket lifecycle must match FR-006 from 001 spec (0600 permissions, stale detection, running-instance check)

**Testing Discipline — Zero-Intrusion Principle**:
- Tests MUST NOT require system-level modifications (no ~/.asoundrc, no modprobe, no PulseAudio config, no root permissions).
- Test infrastructure (virtual audio devices, configuration files, mock services) MUST be self-contained within the project repository.
- Environment variables that affect test behavior MUST be scoped to test execution only — they MUST NOT leak into `cargo build`, `cargo run`, or any non-test cargo invocation.
- A developer MUST be able to `git clone` and `cargo test` without any environment setup beyond Rust toolchain and system audio libraries.
See also: Constitution, Development Process § Strive for Ideal Implementation.
**Rationale**: Testing infrastructure that modifies the user's home directory, requires kernel modules, or sets global environment variables creates hidden state that:
- Causes unexpected failures when the state is absent or stale
- Pollutes the developer's environment with test artifacts
- Makes CI setup fragile and non-reproducible
- Violates the principle of least surprise for contributors

The project-local ALSA null device configuration (`ci/alsa-null.conf`) and
the in-test `ALSA_CONFIG_PATH` setup via `audio_device_available()` exemplify
this principle: the virtual audio device exists only for the lifetime of a test
process, has zero impact on other system audio applications, and requires
no setup beyond `git clone`.
**Scale/Scope**: Single-user desktop; fixes and completes existing MVP

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Compliance |
|-----------|-----------|
| I. Format Universality | ✅ No change — WAV built-in, extension architecture unchanged |
| II. Source Agnosticism | ✅ No change — Source trait unchanged |
| III. Zero-Overhead Extensibility | ⚠️ US4/FR-006 explicitly restores static dispatch for built-in decoders, addressing the regression from T061 |
| IV. Cross-Platform & Form-Factor | △ macOS/Windows output deferred per spec assumptions |
| V. Unobtrusive Presence | ✅ Signal cleanup + IPC lifecycle improve background behavior |
| VI. Privacy by Design | ✅ No network; no change |
| VII. Beyond Local vs. Cloud | ✅ No change |

Architecture Directives:
- A4 (Extreme Performance): ⚠️ FR-006 restores static dispatch, addressing prior regression
- A7 (Comprehensive Observability): ✅ tracing spans already in place
- A8 (Universal Audio Content): ✅ No change

Development Process:
- Test-First (NON-NEGOTIABLE): ⚠️ Tests must be written before code for all changes. Integration tests in US3 must precede implementation.

## Project Structure

### Documentation (this feature)

```text
specs/002-mvp-audio-output/
├── spec.md              # Feature specification
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── ipc-lifecycle.md # IPC server lifecycle contract
└── tasks.md             # Phase 2 output (/speckit.tasks)
```

### Source Code

No new crates. Changes target existing files in the 001 workspace:

```text
crates/
├── around-engine/
│   ├── src/
│   │   ├── pipeline.rs     # Wire cpal output stream, add device_lost check
│   │   ├── config.rs       # Restore static dispatch for WAV decoder
│   │   └── ipc.rs          # Socket lifecycle improvements (already done)
│   └── Cargo.toml          # No new deps
├── around-cli/
│   └── src/
│       └── main.rs         # cmd_play: start IPC server, full signal cleanup
└── around-core/            # No changes

tests/
├── integration/
│   └── ipc_control.rs      # Extended: IPC lifecycle tests
└── contract/               # No changes
```

**Structure Decision**: Same monorepo layout as 001. No new crates. All changes are in-package fixes and integrations.

## Complexity Tracking

No constitution violations requiring justification. The prior regression (dyn dispatch) is being fixed, not introduced.
