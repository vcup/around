# Implementation Plan: Platform-Native IPC with TCP Fallback

**Branch**: `004-platform-native-ipc` | **Date**: 2026-05-31 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `specs/004-platform-native-ipc/spec.md`

## Summary

Replace the current TCP-only IPC fallback with a proper multi-transport IPC system. Linux/macOS use Unix domain sockets at platform-standard paths (`$XDG_RUNTIME_DIR/around/`), Windows uses named pipes at `\\.\pipe\around`, and all platforms support TCP/UDP for cross-host communication. Protobuf becomes the primary wire format with JSON fallback. **Scope: IPC transport infrastructure only. Multi-device synchronized playback is a separate future feature built on top of this system.**

## Technical Context

**Language/Version**: Rust stable 1.95 (edition 2021), pinned via `rust-toolchain.toml`

**Primary Dependencies**:
- `tokio` (net, io-util, rt, sync, macros) — async I/O for TCP/UDP/Unix sockets
- `prost` + `prost-build` — Protobuf code generation (new, feature `ipc-protobuf`)
- `serde_json` — JSON fallback codec (existing)
- `tokio` `windows` feature — async named pipe I/O via `tokio::net::windows::named_pipe` (already in Cargo.toml via `net` feature on Windows targets)

**Storage**: N/A (socket file, port file, named pipe — ephemeral OS resources)

**Testing**: `cargo test --workspace` with `AROUND_FORCE_TCP=1` and `RUST_TEST_THREADS=1` for deterministic cross-platform IPC tests. Platform-native transport tests gated behind `#[cfg(unix)]` / `#[cfg(windows)]`.

**Target Platform**: Linux (ALSA/PulseAudio + Unix sockets), macOS (CoreAudio + Unix sockets), Windows (WASAPI + named pipes). Cross-host TCP/UDP on all platforms.

**Project Type**: Rust workspace — `around-engine` (lib), `around-cli` (bin).

**Performance Goals**: <100ms IPC response via native transport; <10ms UDP datagram processing; <500ms cleanup on exit; <2s TCP fallback startup.

**Constraints**:
- Zero network by default (Constitution VI) — cross-host must be explicitly opted in
- Platform-specific code behind `#[cfg]` (Constitution IV)
- Protobuf: feature-gated (`ipc-protobuf`) to avoid bloat for builds that don't need it
- Windows named pipes use `tokio::net::windows::named_pipe` — fully async, no `spawn_blocking` needed
- All transports share one `IpcCodec` trait and one command dispatch
- TCP and UDP are independently enabled — they run concurrently, may share port numbers

**Scale/Scope**: Single-user local player; optional cross-host for LAN remote control. Multi-device sync is a separate future feature.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Compliance |
|-----------|-----------|
| I. Format Universality | ✅ No change |
| II. Source Agnosticism | ✅ No change |
| III. Zero-Overhead Extensibility | ✅ `IpcCodec` trait uses static dispatch; transports selected at compile time via `#[cfg]` and `ipc-protobuf` feature flag |
| **IV. Cross-Platform & Form-Factor** | ✅ Platform-native transports (Unix sockets, named pipes) + cross-platform TCP/UDP. All `#[cfg]`-gated. Cross-host enables headless server operation. |
| V. Unobtrusive Presence | ✅ Local IPC enables background engine + CLI control |
| **VI. Privacy by Design** | ✅ FR-020: cross-host explicitly opt-in, disabled by default. Zero network unless user enables `--ipc-listen-tcp` or `--ipc-listen-udp` |
| VII. Beyond Local vs. Cloud | △ Cross-host IPC bridges local→remote; full library/streaming deferred to Phase 3 roadmap |

Architecture Directives:
- A4 (Extreme Performance): ✅ Protobuf + datagram UDP for low-overhead IPC; static dispatch via `IpcCodec` trait
- A7 (Comprehensive Observability): ✅ tracing spans on all transport listeners and per-connection handlers

**No violations requiring justification.**

## Project Structure

### Documentation (this feature)

```text
specs/004-platform-native-ipc/
├── plan.md              # This file
├── research.md          # Technology decisions and tradeoffs
├── data-model.md        # Entities, state machines, relationships
├── quickstart.md        # Developer setup and testing guide
├── contracts/           # IPC command schema
│   └── ipc.proto        # Protobuf schema for IpcCommand/IpcResponse
└── tasks.md             # Implementation tasks
```

### Source Code (repository root)

```text
crates/around-engine/src/ipc/
├── mod.rs              # Transport selection + IpcConfig + serve() + run_ipc_server
├── transport_unix.rs   # #[cfg(unix)] Unix socket server + path resolution
├── transport_win.rs    # #[cfg(windows)] Named pipe server (async via tokio)
├── transport_tcp.rs    # TCP (local fallback + remote cross-host)
├── transport_udp.rs    # UDP (low-latency IPC commands)
├── codec.rs            # IpcCodec trait + JsonLineCodec + ProtoCodec
├── types.rs            # PlaybackState, IpcCommand, IpcResponse
└── handlers.rs         # Command dispatch
```

**Structure Decision**: Extract IPC into a sub-module to separate concerns: transport implementations (per-platform), codec (format-agnostic), types (shared), and handlers (pure logic). This replaces the single monolithic `ipc.rs` (~400 lines).

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| Sub-module extraction | 4 transport impls + 2 codecs + handlers exceed single-file maintainability | Single `ipc.rs` would be ~800+ lines — unmaintainable |
| Named pipe transport | Windows named pipes require platform-specific API. | tokio's `net` feature (already enabled) provides `tokio::net::windows::named_pipe` — fully async, no extra dependencies. Raw Win32 FFI via `windows-sys` was rejected because tokio handles it internally. |
| `prost-build` in build.rs | Protobuf codegen requires `.proto` compilation at build time | `prost-reflect` (runtime) would add runtime overhead; build-time codegen is standard |
