# Tasks: Platform-Native IPC with TCP Fallback

**Input**: Design documents from `specs/004-platform-native-ipc/`

**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/

**Tests**: Test tasks are included per Spec US5 ("Tests Cover All Transports") requirement.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Extract IPC module structure, add dependencies, create protobuf build infrastructure.

- [X] T001 Add `prost` and `prost-build` dependencies to workspace `Cargo.toml` and `crates/around-engine/Cargo.toml` behind `ipc-protobuf` feature flag
- [X] T002 [P] Verify `tokio::net::windows::named_pipe` types are available on Windows targets via the existing `net` feature — no additional dependency needed
- [X] T003 [P] Create `crates/around-engine/build.rs` for protobuf code generation using `prost-build` — compile `proto/ipc.proto`, output to `src/ipc/gen/ipc.rs`
- [X] T004 [P] Copy `specs/004-platform-native-ipc/contracts/ipc.proto` to `crates/around-engine/proto/ipc.proto`
- [X] T005 Create IPC sub-module directory `crates/around-engine/src/ipc/` with placeholder `mod.rs`
- [X] T006 Move `crates/around-engine/src/ipc_types.rs` → `crates/around-engine/src/ipc/types.rs` and update `crates/around-engine/src/lib.rs` module path
- [X] T007 Move `crates/around-engine/src/ipc_codec.rs` → `crates/around-engine/src/ipc/codec.rs` and update `crates/around-engine/src/lib.rs` module path
- [X] T008 Move command handlers from `crates/around-engine/src/ipc.rs` to `crates/around-engine/src/ipc/handlers.rs`

**Checkpoint**: IPC module structure in place. All existing tests still pass.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Define `IpcCodec` trait, `IpcConfig` struct, `AcceptStream` trait, `TransportState` enum. Refactor `run_ipc_server` to accept config.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

- [X] T009 Define `IpcCodec` trait in `crates/around-engine/src/ipc/codec.rs` — `read_command(&self, reader: &mut BufReader<R>) -> io::Result<IpcCommand>` (async) and `write_response(&self, writer: &mut W, resp: &IpcResponse) -> io::Result<()>` (async). All transports use async I/O — no sync variants needed (Windows named pipes are async via tokio).
- [X] T010 [P] Implement `IpcCodec` for `JsonLineCodec` in `crates/around-engine/src/ipc/codec.rs` — same logic as existing
- [X] T011 [P] Define `IpcConfig` struct in `crates/around-engine/src/ipc/mod.rs`: `tcp_bind: Option<SocketAddr>`, `udp_bind: Option<SocketAddr>`, `enable_platform_native: bool`. Implement `Default` (remote disabled, platform-native enabled).
- [X] T012 [P] Define `TransportState` enum in `crates/around-engine/src/ipc/mod.rs`: `Stopped | Binding | Listening | ShuttingDown | BindFailed`. Used by each transport to report lifecycle.
- [X] T013 [P] Define `AcceptStream` trait in `crates/around-engine/src/ipc/mod.rs` — generic over `UnixListener` (unix) and `TcpListener` (cross-platform): `type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static`, `async fn accept(&self) -> io::Result<Self::Stream>`.
- [X] T014 Implement `serve()` generic accept loop in `crates/around-engine/src/ipc/mod.rs` — takes `&Arc<Engine>`, `&Arc<Mutex<PlaybackState>>`, `L: AcceptStream`. Spawns `handle_connection` per stream. Checks `engine.is_shutdown()` to exit.
- [X] T015 Refactor `run_ipc_server` in `crates/around-engine/src/ipc/mod.rs` to accept `IpcConfig` parameter. Default config restores current behavior (platform-native + TCP fallback on localhost).

**Checkpoint**: Foundation traits and types ready. All existing 88 tests must still pass.

---

## Phase 3: User Story 1 — Unix Domain Sockets (Priority: P1) 🎯

**Goal**: `around play` creates a Unix socket at `$XDG_RUNTIME_DIR/around/around.sock` (Linux) or `$TMPDIR/around.sock` (macOS) with 0600 permissions. CLI connects via Unix socket. Falls back to `/tmp/around.sock` if platform-standard path unavailable.

**Independent Test**: Start playback on Linux, verify socket at platform-standard path with 0600 perms, run `around status` within 100ms.

### Implementation for User Story 1

- [X] T016 [US1] Implement `run_unix_socket_server` in `crates/around-engine/src/ipc/transport_unix.rs` — `#[cfg(unix)]` gated. Resolve socket path: `$XDG_RUNTIME_DIR/around/` → `$TMPDIR/` → `/tmp/`. Set 0600 perms. Running-instance check via `UnixStream::connect`. Implement `AcceptStream` for `UnixListener`.
- [X] T017 [US1] Implement socket path resolution helper in `crates/around-engine/src/ipc/transport_unix.rs`: `resolve_socket_dir()` returns the best available path (Linux: `$XDG_RUNTIME_DIR/around/`, macOS: `$TMPDIR/around.sock`, fallback: `/tmp/around.sock`). Create parent directory with 0700 perms if needed.
- [X] T018 [US1] Wire transport selection in `run_ipc_server`: if `enable_platform_native` and `#[cfg(unix)]`, try Unix socket → fall back to TCP localhost. If remote TCP/UDP is configured, start those concurrently.

**Checkpoint**: Unix socket IPC works on Linux and macOS. Falls back to TCP if socket dir is unavailable.

---

## Phase 4: User Story 2 — Windows Named Pipes (Priority: P1)

**Goal**: `around play` creates a named pipe at `\\.\pipe\around`. CLI connects via named pipe.

**Independent Test**: Start playback on Windows, verify named pipe active, run `around status` within 100ms.

### Implementation for User Story 2

- [X] T019 [US2] Implement `run_named_pipe_server` in `crates/around-engine/src/ipc/transport_win.rs` — `#[cfg(windows)]` gated. Use `tokio::net::windows::named_pipe::ServerOptions::new().first_pipe_instance(true).create()` to bind at `\\.\pipe\around`. Accept loop via `server.accept()` — fully async. Each accepted connection is a `NamedPipeServer` that implements `AsyncRead + AsyncWrite`.
- [X] T020 [US2] Implement `handle_pipe_connection` in `crates/around-engine/src/ipc/transport_win.rs` — async, uses `IpcCodec::read_command` / `write_response` directly on `NamedPipeServer`. Wire into the shared `serve()` loop via `AcceptStream` trait.

**Checkpoint**: Named pipe IPC works on Windows. Module `#[cfg]`-gated so Linux/macOS builds are unaffected.

---

## Phase 5: User Story 3 — TCP Fallback (Priority: P2)

**Goal**: When native transport fails, engine falls back to TCP on `127.0.0.1:0`. Port file at `<temp>/around.port` for discovery. `AROUND_FORCE_TCP` env var forces TCP for testing.

**Independent Test**: `AROUND_FORCE_TCP=1`, start engine, verify port file, connect via TCP.

### Implementation for User Story 3

- [X] T023 [US3] Implement `run_tcp_server` in `crates/around-engine/src/ipc/transport_tcp.rs` — cross-platform. Bind `127.0.0.1:0`, write port to `<temp>/around.port`. Running-instance check via port file detection. Implement `AcceptStream` for `TcpListener`.
- [X] T024 [US3] Implement port file helpers in `crates/around-engine/src/ipc/transport_tcp.rs`: `port_file_path()` → `std::env::temp_dir().join("around.port")`, `read_port_file()`, `write_port_file()`.
- [X] T025 [US3] Wire `AROUND_FORCE_TCP` env var check in `run_ipc_server` — if set, skip native transports and use TCP only, regardless of `IpcConfig`.

**Checkpoint**: TCP fallback works on all platforms. `AROUND_FORCE_TCP=1` enables deterministic cross-platform testing.

---

## Phase 6: User Story 4 — CLI Auto-Detects Transport (Priority: P2)

**Goal**: CLI automatically discovers the active transport without user configuration.

**Independent Test**: Start engine with native transport, run `around status` — auto-detects. Force TCP, restart, run `around status` — auto-detects port file.

### Implementation for User Story 4

- [X] T026 [US4] Implement transport auto-detection in `crates/around-cli/src/main.rs` (`send_ipc_command`): try Unix socket connect (`#[cfg(unix)]`) → try named pipe open (`#[cfg(windows)]`) → read port file + TCP connect. Return "no engine running" if all fail.
- [X] T027 [US4] Add `--remote <host>:<port>` CLI flag to `crates/around-cli/src/main.rs` — when set, `send_ipc_command` bypasses local discovery and connects directly via TCP.
- [X] T028 [US4] Add `--ipc-listen-tcp <addr>:<port>` and `--ipc-listen-udp <addr>:<port>` CLI flags to `crates/around-cli/src/main.rs` using clap. Both optional and independent.
- [X] T029 [US4] Update `cmd_play` in `crates/around-cli/src/main.rs` to construct `IpcConfig` from CLI flags, pass to `run_ipc_server`. Default config uses platform-native + TCP fallback.

**Checkpoint**: CLI works transparently with any transport. `--remote` enables remote control.

---

## Phase 7: User Story 5 — Tests Cover All Transports (Priority: P3)

**Goal**: All IPC tests pass on all platforms, exercise native transports on respective OS.

**Independent Test**: `AROUND_FORCE_TCP=1 cargo test --workspace` passes. `cargo test --test ipc_control` passes using native transports.

### Tests for User Story 5

- [X] T030 [P] [US5] Write Unix socket lifecycle test in `crates/around-engine/tests/ipc_control.rs` — start engine, verify socket at platform-standard path with 0600 perms, stop, verify socket deleted. `#[cfg(unix)]` gated.
- [X] T031 [P] [US5] Write named pipe lifecycle test in `crates/around-engine/tests/ipc_control.rs` — start engine, verify pipe active, stop, verify pipe closed. `#[cfg(windows)]` gated.
- [X] T032 [P] [US5] Write TCP fallback test — set `AROUND_FORCE_TCP=1`, verify port file created, connect via TCP, stop, verify port file deleted.
- [X] T033 [P] [US5] Write transport auto-detection test — start engine with native transport, verify CLI auto-discovers.
- [X] T034 [P] [US5] Write `IpcConfig` default test — verify default config produces platform-native + TCP fallback behavior.
- [X] T035 [US5] Fix 8 gated IPC tests in `crates/around-engine/tests/ipc_control.rs` — replace `cfg(any())` with transport-aware test logic using `std::env::temp_dir()` paths and `AROUND_FORCE_TCP`.

**Checkpoint**: Full test coverage — native transports tested on respective platforms, TCP fallback tested everywhere.

---

## Phase 8: User Story 6 — Cross-Host Remote Control (Priority: P2)

**Goal**: Engine accepts remote TCP connections when `--ipc-listen-tcp` is specified.

**Independent Test**: Start engine with `--ipc-listen-tcp 127.0.0.1:19123`, connect via `--remote 127.0.0.1:19123`, send `status` command.

### Implementation for User Story 6

- [X] T036 [US6] Implement cross-host TCP listener in `crates/around-engine/src/ipc/transport_tcp.rs` — when `IpcConfig.tcp_bind` is set, bind an additional TCP listener on that address (alongside the local `127.0.0.1:0` listener). Uses same `serve()` loop.
- [X] T037 [US6] Implement env var support — `AROUND_IPC_TCP_BIND=0.0.0.0:9123` populates `IpcConfig.tcp_bind`. `AROUND_IPC_UDP_BIND=0.0.0.0:19124` populates `IpcConfig.udp_bind`. Parse in `IpcConfig::from_env()`.
- [X] T038 [US6] Write cross-host connectivity test in `crates/around-engine/tests/ipc_control.rs` — start engine with `IpcConfig { tcp_bind: Some("127.0.0.1:0"), ..Default::default() }`, get assigned port, connect remotely, send command, verify response.

**Checkpoint**: Cross-host TCP works. Engine defaults to local-only per Constitution VI.

---

## Phase 9: User Story 7 — UDP Transport Infrastructure (Priority: P3)

**Goal**: Engine accepts UDP datagrams as IPC commands. Foundation for future low-latency features (NOT implementing multi-device sync).

**Independent Test**: Start engine with `--ipc-listen-udp 127.0.0.1:19124`, send valid `status` IPC command via UDP datagram, verify response.

### Implementation for User Story 7

- [X] T039 [US7] Implement `run_udp_server` in `crates/around-engine/src/ipc/transport_udp.rs` — when `IpcConfig.udp_bind` is set, bind `UdpSocket` to that address. Each datagram is one complete IPC message. Use `IpcCodec` to parse commands. Responses sent to sender's `SocketAddr`. TCP and UDP may share the same port.
- [X] T040 [US7] Implement UDP datagram size validation — reject datagrams > 65507 bytes (max UDP payload). Log warning for > 512 bytes (safe limit for non-fragmented delivery).
- [X] T041 [US7] Write UDP send/receive test in `crates/around-engine/tests/ipc_control.rs` — start engine with UDP enabled, send valid IPC command, verify response.
- [X] T042 [US7] Write UDP malformed datagram test — send garbage data, verify engine logs warning and continues without crashing.

**Checkpoint**: UDP transport works for IPC commands. Foundation ready for future real-time features. Multi-device sync is **out of scope** for this phase.

---

## Phase 10: Protobuf Codec (Priority: P3)

**Goal**: Protobuf replaces JSON as primary wire format when `ipc-protobuf` feature is enabled.

**Independent Test**: Build with `--features ipc-protobuf`, start engine, send protobuf-encoded command, verify response is protobuf.

### Implementation for Protobuf Codec

- [X] T043 [P] Implement `ProtoCodec` struct in `crates/around-engine/src/ipc/codec.rs` — `#[cfg(feature = "ipc-protobuf")]` gated. Implements `IpcCodec` trait. Uses length-delimited framing (varint prefix + serialized message bytes).
- [X] T044 [P] Implement codec auto-selection in `crates/around-engine/src/ipc/mod.rs` — `fn select_codec() -> Box<dyn IpcCodec>`: if `ipc-protobuf` enabled, use `ProtoCodec`; otherwise `JsonLineCodec`. Auto-detect incoming format from first byte: `0x0a` = protobuf, `{` = JSON.
- [X] T045 [P] Write protobuf round-trip test — encode `IpcCommand` via proto, decode via proto, verify identical. Encode via proto, decode via JSON fallback, verify identical (FR-013 compliance).
- [X] T046 Wire `build.rs` output into `crates/around-engine/src/ipc/mod.rs` via module path for generated proto code.

**Checkpoint**: Protobuf codec functional and feature-gated. JSON fallback always available.

---

## Phase 11: Polish & Cross-Cutting Concerns

**Purpose**: Final validation, integrate `platform-transport-v2` branch work, CI verification.

- [X] T047 [P] Update `crates/around-engine/src/lib.rs` — `pub mod ipc` re-exports from `ipc/mod.rs`. Re-export `IpcConfig` publicly.
- [X] T048 [P] Run `cargo fmt` and `cargo clippy --workspace -- -D warnings` — fix all warnings
- [X] T049 Run `cargo test --workspace` — all tests pass on Linux (expected: 88+ new tests)
- [X] T050 Run `cargo test --workspace` with `AROUND_FORCE_TCP=1` and `RUST_TEST_THREADS=1` — TCP fallback tests pass on all platforms
- [X] T051 Integrate `platform-transport-v2` branch work (Unix socket + named pipe prototypes) into the new `ipc/` module structure, adapting to `IpcConfig` and async `IpcCodec` trait (tokio named pipe API replaces raw Win32)
- [X] T052 Verify CI green on Linux, macOS, Windows via GitHub Actions
- [X] T053 Mark all completed tasks as `[X]`

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies
- **Foundational (Phase 2)**: Depends on Setup — BLOCKS all stories
- **US1 — Unix Sockets (Phase 3)**: Depends on Foundational
- **US2 — Named Pipes (Phase 4)**: Depends on Foundational — parallel with US1
- **US3 — TCP Fallback (Phase 5)**: Depends on Foundational — parallel with US1/US2
- **US4 — CLI Discovery (Phase 6)**: Depends on US1+US2+US3 (needs all transports)
- **US5 — Tests (Phase 7)**: Depends on US1+US2+US3+US4
- **US6 — Cross-Host (Phase 8)**: Depends on US3 (TCP) + US4 (CLI flags)
- **US7 — UDP (Phase 9)**: Depends on Foundational (IpcCodec) + US4 (CLI flags)
- **Protobuf (Phase 10)**: Depends on Foundational (IpcCodec trait)
- **Polish (Phase 11)**: Depends on all desired stories

### Parallel Opportunities

- **Phase 1**: T002, T003, T004 all [P]
- **Phase 2**: T010-T013 all [P]
- **Phase 3-5**: US1 (T016-T018), US2 (T019-T022), US3 (T023-T025) run in parallel
- **Phase 7**: T030-T034 all [P]
- **Phase 10**: T043-T045 all [P]

## Implementation Strategy

### MVP First (US1 + US2 + US3 Only)

1. Phase 1: Setup — module extraction + dependencies
2. Phase 2: Foundational — IpcCodec trait + IpcConfig + AcceptStream
3. Phase 3: US1 — Unix sockets (Linux/macOS)
4. Phase 4: US2 — Named pipes (Windows)
5. Phase 5: US3 — TCP fallback (all platforms)
6. **STOP and VALIDATE**: Local IPC works on all three platforms
7. This is the production IPC replacement

### Incremental Delivery

- **Sprint 1**: Phase 1-3 (Unix sockets) — Linux/macOS native IPC
- **Sprint 2**: Phase 4 (Named pipes) — Windows native IPC
- **Sprint 3**: Phase 5-7 (TCP + CLI + Tests) — cross-platform completeness
- **Sprint 4**: Phase 8-10 (Cross-host + UDP + Protobuf) — advanced features

## Notes

- [P] tasks = different files, no dependencies — run in parallel
- The `platform-transport-v2` branch contains working prototypes of T016-T018 (Unix socket) and T019-T020 (named pipes) — adapt these into the new `ipc/` module. Note: the branch uses raw Win32 named pipes; the implementation should use `tokio::net::windows::named_pipe` instead
- Constitution VI: cross-host defaults to OFF
- Constitution IV: platform-specific code `#[cfg]`-gated
- TCP and UDP may share the same port number (different protocol stacks)
- IPC server lifecycle is tied to application exit, NOT `stop` (which only halts playback)
