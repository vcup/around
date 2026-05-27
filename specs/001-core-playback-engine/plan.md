# Implementation Plan: Core Audio Playback Engine

**Branch**: `001-core-playback-engine` | **Date**: 2026-05-26 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `specs/001-core-playback-engine/spec.md`

## Summary

Build the foundational audio playback engine for `around`: format decoding via a built-in WAV decoder and runtime-loadable FFmpeg-backed extensions, local file source abstraction, cross-platform audio output, CLI control surface with foreground playback + local IPC, and a zero-overhead extension system configured in KDL. This feature establishes the constitutional contract traits (`Decoder`, `Source`) that all future features build upon.

## Technical Context

**Language/Version**: Rust (stable, edition 2021)

**Primary Dependencies**:
- `symphonia` — WAV decoding (pure Rust, multi-format capable for future)
- `libloading` — runtime shared library loading for extensions
- `kdl-rs` — KDL configuration parsing (manual tree traversal; strict core validation + extension config passthrough)
- `tracing` — structured logging (spans, events, configurable levels)
- `cpal` — cross-platform audio output abstraction (CoreAudio/PulseAudio/WASAPI)
- `tokio` — async runtime for IPC (Unix domain sockets / named pipes)

**Storage**: N/A (no persistent state in MVP; local files read-only)

**Testing**: `cargo test` (unit + contract + integration), `cargo doc --test` (literate programming doc-tests)

**Target Platform**: Linux (PulseAudio/ALSA), macOS (CoreAudio), Windows (WASAPI)

**Project Type**: Rust workspace — library crates (`around-core`, `around-engine`) + CLI binary (`around-cli`)

**Performance Goals**: <10ms source-to-output latency, <50MB RSS idle, <5% single-core CPU during playback, <500ms cold start, <10MB stripped headless binary

**Constraints**: Zero network connectivity for core playback; extension loading must match static-link performance; WAV is the sole built-in decoder; KDL config with unknown-key rejection; no `dyn Trait` in hot paths

**Scale/Scope**: Single-user, single-process MVP; 3 decoder families (WAV built-in, FFmpeg extension, minimal PCM test decoder for contracts); local filesystem source only

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

### Core Principles (I–VII)

| Principle | Compliance |
|-----------|-----------|
| I. Format Universality | ✅ WAV built-in + Decoder trait for all formats via extensions |
| II. Source Agnosticism | ✅ Source trait abstracts local filesystem; HTTP/etc. deferred to extensions |
| III. Zero-Overhead Extensibility | ✅ Compile-time static dispatch for built-in decoder; `libloading` + shim for runtime extensions with no per-call overhead (symbols resolved once at load) |
| IV. Cross-Platform & Form-Factor | ✅ `cpal` abstracts audio output; CLI is the initial form factor; headless and GUI deferred |
| V. Unobtrusive Presence | ✅ Engine as library crate with IPC separation from CLI; foreground process + socket control from separate terminal |
| VI. Privacy by Design | ✅ Zero network connections for local playback; no telemetry |
| VII. Beyond Local vs. Cloud | ✅ Architecture supports future remote sources via Source trait; local-only by default, extensible |

### Architecture Directives

| Directive | Compliance |
|-----------|-----------|
| A1. Powerful Configuration | ✅ KDL layered config: defaults → file → CLI flags; unknown keys rejected |
| A2. Comprehensive Media Library | ⏸ Deferred — no library/DB in MVP; architecture prepared via Source trait |
| A3. Protocol Diversity | ⏸ Deferred — CLI + local IPC only in MVP; REST/gRPC/MPRIS deferred |
| A4. Extreme Performance | ✅ Targets: <10ms, <50MB, <5% CPU, <500ms startup, <10MB binary |
| A5. Data Resilience | ⏸ Deferred — no persistent state in MVP |
| A6. Federated Cloud Backend | ⏸ Deferred |
| A7. Comprehensive Observability | ✅ Structured logging via `tracing` with configurable levels; metrics/profiling deferred |
| A8. Universal Audio Content | ✅ Content type is a tag in Track entity; no artificial music-only limitation |

### Development Process

| Rule | Compliance |
|------|-----------|
| Test-First (NON-NEGOTIABLE) | ✅ TDD cycle: write contract tests → fail → implement → pass |
| CI Gates | ⏸ CI pipeline deferred (constitution gates defined for future setup) |

## Project Structure

### Documentation (this feature)

```text
specs/001-core-playback-engine/
├── spec.md              # Feature specification
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   ├── decoder-trait.md
│   ├── source-trait.md
│   └── ipc-commands.md
└── tasks.md             # Phase 2 output (/speckit.tasks)
```

### Source Code (repository root)

```text
crates/
├── around-core/            # Public traits & types: Decoder, Source, Metadata, PlaybackState, Error
│   ├── src/
│   │   ├── lib.rs
│   │   ├── decoder.rs      # Decoder trait definition
│   │   ├── source.rs       # Source trait definition
│   │   ├── metadata.rs     # Metadata struct & trait
│   │   ├── state.rs        # PlaybackState enum
│   │   └── error.rs        # Error types (AroundError)
│   └── Cargo.toml
├── around-engine/           # Playback engine: pipeline, IPC, extension manager
│   ├── src/
│   │   ├── lib.rs
│   │   ├── pipeline.rs     # Source → Decoder → Output pipeline
│   │   ├── ipc.rs          # Local socket/named pipe command interface
│   │   ├── extensions.rs   # Extension loading, registry, search paths
│   │   ├── config.rs       # KDL config loading (defaults → file → CLI)
│   │   └── platform/       # Per-platform audio output backends
│   │       ├── mod.rs
│   │       └── linux.rs / macos.rs / windows.rs
│   └── Cargo.toml
├── around-cli/              # CLI binary
│   ├── src/
│   │   └── main.rs         # CLI argument parsing, IPC client to engine
│   └── Cargo.toml
├── around-codec-wav/        # Built-in WAV decoder (implements Decoder trait)
│   ├── src/
│   │   └── lib.rs
│   └── Cargo.toml
├── around-codec-test-pcm/   # Minimal PCM decoder for contract testing
│   ├── src/
│   │   └── lib.rs
│   └── Cargo.toml
└── around-source-file/      # Local filesystem Source implementation
    ├── src/
    │   └── lib.rs
    └── Cargo.toml

tests/
├── contract/                # Trait contract tests
│   ├── decoder.rs
│   └── source.rs
├── integration/             # End-to-end playback tests
│   └── playback_pipeline.rs
└── unit/                    # Per-crate unit tests (mirrors crates/)

examples/
├── example.wav              # Test fixture
└── simple_decoder/          # Demo of writing a Decoder extension
    ├── Cargo.toml
    └── src/lib.rs
```

**Structure Decision**: Rust workspace with individual crates per concern. `around-core` is the zero-dependency traits crate that extension authors depend on. `around-engine` assembles the pipeline using trait objects at the non-hot boundary (setup only). All hot-path dispatch is statically resolved.

## Complexity Tracking

No constitution violations. All deferred directives (A2, A3, A5, A6) are explicitly acknowledged as subsequent features, not violations.
