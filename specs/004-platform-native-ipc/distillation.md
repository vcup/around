# 004-platform-native-ipc — Design Philosophy & Requirements Distillation

## Core Design Principle

**Native-first, TCP-fallback, cross-host opt-in.** The IPC system elevates the platform's native IPC mechanism as the primary transport. Unix domain sockets on Linux/macOS and named pipes on Windows provide filesystem-level access control (0600 permissions), zero-TCP-stack-overhead, and idiomatic platform conventions. TCP on 127.0.0.1 serves only as a transparent fallback when native binding fails — not as the default. Cross-host TCP/UDP is explicitly opt-in (Constitution VI: Privacy by Design).

## Architecture

```
                    ┌─────────────────────────────┐
                    │       IpcCommand / IpcResponse
                    │       (serde JSON or prost)   │
                    ├─────────────────────────────┤
                    │         IpcCodec trait        │
                    │    JsonLineCodec / ProtoCodec  │
                    ├─────────────────────────────┤
                    │    serve() / handle_connection │
                    ├─────────────────────────────┤
                    │     Transport Selection       │
                    │  ┌─────────┬────────┬──────┐  │
                    │  │  Unix   │ Named  │ TCP  │  │
                    │  │ Socket  │ Pipe   │Fback │  │
                    │  └─────────┴────────┴──────┘  │
                    │  ┌─────────────────────────┐  │
                    │  │ Remote TCP │ Remote UDP │  │
                    │  │ (opt-in)   │ (Phase 9)  │  │
                    │  └─────────────────────────┘  │
                    └─────────────────────────────┘
```

## Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| Native-first, TCP fallback | Spec FR-003. Native IPC is faster, respects filesystem permissions, and is the idiomatic choice. TCP is a universal safety net. |
| No env-var transport control | `IpcConfig` struct provides type-safe, testable configuration. `AROUND_FORCE_TCP` was a testing artifact — removed. |
| `check_running_instance` configurable | Default `true` for production; tests set `false` to avoid global path conflicts. |
| `unix_socket_path` configurable | Enables parallel-safe Unix socket tests with unique per-test paths. |
| Shared `ExtensionManager` | One instance across all transports prevents decoder state inconsistency. |
| `JsonLineCodec` default, ProtoCodec feature-gated | JSON Lines is universally accessible; Protobuf adds schema evolution and compact encoding for future use. |
| 16-line skip limit in codec | Prevents memory-exhaustion DoS from infinite empty-line streams. |

## Requirements Coverage

- **FR-001** Unix socket at platform-standard path with 0600 permissions ✅
- **FR-002** Windows named pipe at `\\.\pipe\around` ✅
- **FR-003** Native-first, TCP fallback on 127.0.0.1:0 ✅
- **FR-004** Duplicate startup detection on all transports ✅
- **FR-005** Socket/port file cleanup on exit ✅
- **FR-006/011** CLI transport auto-detection (native → port file) ✅
- **FR-007** `AROUND_FORCE_TCP` replaced by `IpcConfig::enable_platform_native` ✅
- **FR-008** `#[cfg]` gating on all platform code ✅
- **FR-009** ProtoCodec feature-gated (Phase 10 deferred) ⚠️
- **FR-010** Running-instance detection via connect-before-bind ✅
- **FR-012** Integration tests on all platforms ✅
- **FR-013** Protobuf↔JSON round-trip (Phase 10 deferred) ⚠️
- **FR-014** `--ipc-listen-tcp/udp` CLI flags + env vars ✅
- **FR-015** TCP/UDP documented roles ✅
- **FR-016** UDP framing (Phase 9 deferred) ⚠️
- **FR-017** Concurrent transports in separate tokio tasks ✅
- **FR-018** `IpcConfig` public struct ✅
- **FR-019** `--remote` CLI flag ✅
- **FR-020** Cross-host disabled by default ✅

## Platform Support Matrix

| Feature | Linux | macOS | Windows |
|---------|-------|-------|---------|
| Unix domain socket | ✅ | ✅ | N/A |
| Named pipe | N/A | N/A | ✅ |
| TCP local fallback | ✅ | ✅ | ✅ |
| TCP cross-host | ✅ | ✅ | ✅ |
| UDP remote | ⚠️ Phase 9 | ⚠️ Phase 9 | ⚠️ Phase 9 |
| ProtoCodec | ⚠️ Phase 10 | ⚠️ Phase 10 | ⚠️ Phase 10 |
| Unix socket tests | ✅ 5 tests | ✅ same binary | N/A |
| Named pipe tests | N/A | N/A | ⚠️ 0 tests |
| IPC control tests | ✅ TCP mode | ✅ TCP mode | ✅ TCP mode |

## Test Architecture

```
ipc_control.rs      → TCP-only tests (enable_platform_native: false)
                      Protocol correctness is transport-agnostic
                      Unique port files per test for parallel safety

ipc_unix_socket.rs  → Unix socket lifecycle tests (enable_platform_native: true)
                      Unique socket paths per test via IpcConfig::unix_socket_path
                      5 tests: permissions, round-trip, duplicate, stale, resolve

ipc_types_tests.rs  → IpcCommand/IpcResponse/IpcConfig unit tests
                      Isolated from transport — pure serialization/deserialization
