# Feature Specification: Platform-Native IPC with TCP Fallback

**Feature Branch**: `004-platform-native-ipc`

**Created**: 2026-05-31

**Status**: Draft

**Input**: Replace the current TCP-only IPC fallback (adopted during MVP CI stabilization) with a proper multi-transport IPC system. Linux/macOS use Unix domain sockets; Windows uses named pipes; all platforms fall back to TCP if the native transport is unavailable. This consolidates work prototyped on `platform-transport-v2` branch and addresses findings from cross-platform analysis (Constitution Principle IV — platform-specific code must be `#[cfg]`-gated, not removed).

## Clarifications

### Session 2026-05-31

- Q: IPC server lifecycle — does `stop` shutdown the IPC server? → A: No. `stop` only stops playback; the engine then proceeds through normal shutdown flow which cleans up IPC. The IPC server lifecycle is tied to application exit/cleanup, not to the stop command.
- Q: Unix socket location — `/tmp/around.sock` or platform-standard directory? → A: Use platform-standard directories with fallback. Linux: `$XDG_RUNTIME_DIR/around/` (typically `/run/user/<UID>/around/around.sock`). macOS: `$TMPDIR/around.sock` (sandbox-safe). Fallback on any platform: `/tmp/around.sock`. Windows: named pipes already use the standard `\\.\pipe\` namespace.
- Q: IPC wire format — JSON Lines or binary protocol? → A: Protobuf (via `prost`) as primary, with JSON Lines retained as debug/fallback. Protobuf provides compact binary encoding, schema evolution, and strong Rust ecosystem. JSON kept for human debugging and as fallback when protobuf parsing fails. Codec selection is configurable at compile time (feature flag `ipc-protobuf`) and runtime (fallback to JSON).
- Q: Should the IPC system support cross-host (remote) communication? → A: Yes, but disabled by default. When enabled via `--ipc-listen-tcp <addr>:<port>` / `--ipc-listen-udp <addr>:<port>` CLI flags or `AROUND_IPC_TCP_BIND` / `AROUND_IPC_UDP_BIND` env vars, the engine binds TCP and/or UDP on non-localhost interfaces. Both may be enabled simultaneously. Local platform-native IPC (Unix sockets, named pipes) always runs regardless. Cross-host is opt-in for security per Constitution VI (Privacy by Design — zero network by default).
- Q: Should UDP be supported alongside TCP? → A: Yes. TCP provides reliable streaming; UDP provides connectionless low-latency messaging as the foundation for future real-time features (multi-instance coordination, etc.). Both transports may be enabled independently and run concurrently — they are configured with separate addresses and may share the same port number (different protocol stacks).
- Q: Can multiple transports run simultaneously? → A: Yes. Platform-native IPC always runs. TCP and UDP can be enabled independently via CLI/env. All transports share the same `IpcCodec` and command dispatch. This enables hybrid scenarios: local control via Unix socket, remote control via TCP, and low-latency messaging via UDP — all in one engine process.
- Q: What is the scope boundary for this feature? → A: IPC transport system only. Multi-device synchronized playback (US7's multi-instance sync scenario) is a separate feature built on top of the IPC system, NOT implemented in this phase. The UDP transport infrastructure is implemented for IPC commands; the higher-level coordination protocol for multi-room sync is deferred.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Native IPC on Linux and macOS (Priority: P1)

A user runs `around play <file>` on Linux or macOS. The engine creates a Unix domain socket at the platform-standard location (Linux: `$XDG_RUNTIME_DIR/around/around.sock` — typically `/run/user/<UID>/around/around.sock`; macOS: `$TMPDIR/around.sock`; fallback: `/tmp/around.sock`) with 0600 permissions. A second terminal runs `around status` and connects via the Unix socket to retrieve playback state. When the application exits, the socket file is removed during cleanup.

**Why this priority**: Unix sockets are the standard local IPC mechanism on POSIX systems — they are faster than TCP loopback (no TCP stack overhead), respect filesystem permissions, and are the idiomatic choice for local service communication. This is the primary transport for the two most-used developer platforms.

**Independent Test**: Start playback on Linux, run `around status` from a second terminal — response received within 100ms. Verify socket exists at platform-standard location (`$XDG_RUNTIME_DIR/around/around.sock`) with 0600 permissions. Stop playback — socket file deleted within 500ms.

**Acceptance Scenarios**:

1. **Given** Linux or macOS, **When** `around play` starts, **Then** a Unix domain socket is created at the platform-standard location (`$XDG_RUNTIME_DIR/around/` or `$TMPDIR`, with `/tmp` fallback) with 0600 permissions before audio output begins.
2. **Given** playback is active, **When** `around status` runs, **Then** the command connects via the Unix socket and receives a valid JSON response within 100ms.
3. **Given** playback is active, **When** `around stop` runs, **Then** playback stops, the socket is closed, and the socket file is deleted within 500ms.
4. **Given** a stale socket file exists from a crashed process, **When** `around play` starts, **Then** the stale file is detected (connection refused), deleted, and a fresh socket is bound.
5. **Given** an engine instance is already running on the socket, **When** a second `around play` starts, **Then** it detects the live socket, returns an error, and exits without overwriting.
6. **Given** the platform-standard directory is not writable (e.g., container without `$XDG_RUNTIME_DIR`), **When** the engine starts, **Then** Unix socket binding falls back to `/tmp`, or falls back to TCP if `/tmp` is also unavailable.
---

### User Story 2 - Native IPC on Windows (Priority: P1)

A user runs `around play <file>` on Windows. The engine creates a named pipe at `\\.\pipe\around`. A second terminal runs `around status` and connects via the named pipe to retrieve playback state. When playback ends, the pipe server stops accepting new connections.

**Why this priority**: Named pipes are the standard local IPC mechanism on Windows — they are the equivalent of Unix domain sockets, support access control, and are the idiomatic choice. Constitution IV mandates platform-native support on all three OS.

**Independent Test**: Start playback on Windows, run `around status` from a second terminal — response received within 100ms. Verify named pipe is active. Stop playback — pipe server shuts down gracefully.

**Acceptance Scenarios**:

1. **Given** Windows, **When** `around play` starts, **Then** a named pipe listener is created at `\\.\pipe\around` before audio output begins.
2. **Given** playback is active on Windows, **When** `around status` runs, **Then** the command opens the named pipe and receives a valid JSON response within 100ms.
3. **Given** the named pipe cannot be created (e.g., permissions error), **When** the engine starts, **Then** it falls back to TCP transport on localhost.
4. **Given** a client disconnects abruptly (pipe broken), **When** the server handles the disconnection, **Then** it closes the pipe handle without crashing and continues accepting new connections.

---

### User Story 3 - TCP Fallback on All Platforms (Priority: P2)

When the native transport is unavailable (e.g., `/tmp` not writable, named pipe permission denied), the engine falls back to TCP on `127.0.0.1` with a random port. The port number is written to a file at `<system-temp-dir>/around.port` for client discovery. This ensures the engine is always controllable regardless of filesystem or OS limitations.

**Why this priority**: The fallback ensures the IPC system never blocks playback. On constrained environments (containers, CI, restricted user accounts), TCP is always available. This also enables deterministic cross-platform testing via the `AROUND_FORCE_TCP` environment variable.

**Independent Test**: Set `AROUND_FORCE_TCP=1`, start playback, verify the engine writes its port to `<temp>/around.port`, and `around status` connects via TCP.

**Acceptance Scenarios**:

1. **Given** the native transport fails on any platform, **When** the engine starts, **Then** it falls back to TCP on `127.0.0.1:<random-port>` and writes the port to `<temp>/around.port`.
2. **Given** TCP fallback is active, **When** a second instance starts, **Then** it reads the port file, detects the running instance, and returns an error.
3. **Given** `AROUND_FORCE_TCP=1` is set in the environment, **When** the engine starts, **Then** it uses TCP regardless of native transport availability (enables cross-platform test determinism).

---

### User Story 4 - CLI Auto-Detects Transport (Priority: P2)

The CLI (`around status`, `around stop`, etc.) automatically detects which transport the engine is using without explicit configuration. It tries the native transport first (Unix socket on Linux/macOS, named pipe on Windows), then falls back to reading the port file for TCP. This works identically across all platforms.

**Why this priority**: Users should never need to know or specify which IPC transport is active. The CLI must be as seamless as the engine.

**Independent Test**: On Linux, start engine with native transport, run `around status` — succeeds. Force TCP with `AROUND_FORCE_TCP=1`, restart, run `around status` — also succeeds without any CLI changes.

**Acceptance Scenarios**:

1. **Given** the engine is running with native transport on Linux, **When** `around status` runs, **Then** it connects via Unix socket without requiring any transport flag.
2. **Given** the engine is running with TCP fallback on any platform, **When** `around status` runs, **Then** it reads the port file and connects via TCP without requiring any transport flag.
3. **Given** no engine is running, **When** any IPC command runs, **Then** it tries all available transports and reports "no engine running" if none respond.

---

### User Story 5 - Tests Cover All Transports (Priority: P3)

The test suite validates all three transport paths: Unix socket (Linux/macOS), named pipe (Windows), and TCP fallback (all platforms). Tests use `AROUND_FORCE_TCP=1` for cross-platform determinism and test native transports on their respective platforms.

**Why this priority**: Transport bugs are subtle (timing, permissions, handle lifetime) and must be caught in CI. The current TCP-only tests provide baseline coverage; native transport tests add platform-specific validation.

**Independent Test**: CI runs IPC tests on Linux (validates Unix socket path), Windows (validates named pipe path), and with `AROUND_FORCE_TCP=1` (validates TCP fallback on all platforms).

**Acceptance Scenarios**:

1. **Given** Linux CI, **When** `cargo test --test ipc_control` runs, **Then** all IPC tests pass using Unix sockets on Linux, named pipes on Windows, and TCP fallback when `AROUND_FORCE_TCP=1`.
2. **Given** macOS CI, **When** tests run, **Then** IPC tests pass using Unix sockets.
3. **Given** Windows CI, **When** tests run, **Then** IPC tests pass using named pipes.

---


### User Story 6 - Cross-Host Remote Control (Priority: P2)

A user runs `around play --ipc-listen-tcp 0.0.0.0:9123` on a headless server. Another machine on the local network runs `around --remote 192.168.1.5:9123 status` and receives the playback state. The engine's local Unix socket still works for same-machine control. Cross-host communication is disabled by default — it must be explicitly enabled.

**Why this priority**: Constitution VII (Beyond Local vs. Cloud) requires bridging local and remote paradigms. Remote control enables headless server operation (Constitution IV form-factor). Security is paramount — zero network exposure by default (Constitution VI).

**Independent Test**: Start engine with `--ipc-listen-tcp 0.0.0.0:19123`, connect from another machine via `around --remote <host>:19123 status`, verify response.

**Acceptance Scenarios**:

1. **Given** the engine starts without `--ipc-listen`, **When** a remote client attempts to connect, **Then** no server is listening on external interfaces and the connection is refused.
2. **Given** `--ipc-listen-tcp 0.0.0.0:9123`, **When** the engine starts, **Then** a TCP server listens on port 9123 on all interfaces while the local Unix socket/named pipe also operates.
3. **Given** cross-host TCP is active, **When** a remote client sends a `status` command, **Then** the engine returns the current playback state.

---


### User Story 7 - UDP Transport for Low-Latency IPC (Priority: P3)

A user enables UDP transport via `--ipc-listen-udp 0.0.0.0:19123`. The engine binds a UDP socket and processes each received datagram as a complete IPC command (e.g., `status`, `pause`, `stop`). UDP's connectionless, low-overhead nature makes it the foundation for future low-latency features (multi-instance coordination, real-time control) — but those features are built ON TOP of this transport, not implemented here.

**Why this priority**: UDP is the only standard transport that provides connectionless, low-latency messaging suitable for real-time control scenarios. Implementing the UDP transport now (with clear `IpcCodec` boundaries) ensures the IPC system is complete and extensible for future higher-level protocols.

**Independent Test**: Start engine with `--ipc-listen-udp 127.0.0.1:19124`, send a valid `status` IPC command via UDP datagram, verify the response is a correctly-formatted IPC response.

**Acceptance Scenarios**:

1. **Given** the engine starts with `--ipc-listen-udp 127.0.0.1:0`, **When** a well-formed `status` IPC command datagram arrives, **Then** the engine processes it and sends a response to the sender's address.
2. **Given** UDP transport is active, **When** a malformed datagram arrives (garbage bytes, invalid Protobuf/JSON), **Then** the engine ignores it and logs a warning without crashing.
3. **Given** both TCP and UDP are enabled, **When** a command arrives via TCP and another via UDP simultaneously, **Then** both are handled independently without interference.

**Out of scope for this feature**: Multi-instance synchronized playback, UDP multicast discovery, clock synchronization, and distributed state coordination. These are separate features that will use the UDP transport as their foundation.

### Edge Cases

- What happens when a Unix socket file is deleted externally during playback? → Next client connection fails; engine continues playback and logs a warning.
- What happens when the named pipe server receives concurrent connections? → Each connection creates a new pipe instance via `PIPE_UNLIMITED_INSTANCES`, handled in its own tokio task.
- What happens when the TCP port file is deleted externally? → Next CLI command cannot discover the port; returns "no engine running" error.
- What happens when a client connects to a named pipe but sends no data? → The server's read times out or returns EOF; the pipe instance is closed without affecting the server.
- What happens on Windows when the named pipe server fails to bind (e.g., insufficient privileges)? → The engine falls back to TCP transport automatically.
- What happens when a named pipe client connects between server creation and accept (race condition)? → tokio handles this internally — the server treats it as a normal connection.
- What happens when both TCP and UDP are enabled, and a TCP client connects while a UDP datagram arrives simultaneously? → Handled independently — each transport runs in its own tokio task using the shared command dispatch.
- What happens when a UDP datagram exceeds the codec's maximum message size? → The engine truncates and logs a warning; partial commands are rejected at the codec level.
- What happens when `--ipc-listen-tcp` or `--ipc-listen-udp` specifies a port already in use? → The engine logs the error and falls back to local-only IPC (platform-native + TCP fallback).
- What happens when a remote client sends commands without authentication? → Cross-host is trusted-LAN-only in this phase. A future FR will add authentication.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: On Linux, the IPC server MUST create a Unix domain socket at `$XDG_RUNTIME_DIR/around/around.sock` (typically `/run/user/<UID>/around/`), falling back to `/tmp/around.sock` if `$XDG_RUNTIME_DIR` is not set or the directory is not writable. On macOS, use `$TMPDIR/around.sock`, falling back to `/tmp/around.sock`. The socket MUST have 0600 permissions.
- **FR-002**: On Windows, the IPC server MUST create a named pipe at `\\.\pipe\around` with `PIPE_UNLIMITED_INSTANCES` for concurrent connections.
- **FR-003**: If the native transport fails to bind (permissions, path unavailable, API error), the server MUST fall back to TCP on `127.0.0.1:<random-port>` and write the port to `<system-temp-dir>/around.port`.
- **FR-004**: The server MUST detect a running instance on the same transport and reject duplicate startups with a descriptive error.
- **FR-005**: The server MUST clean up transport resources on application exit: delete the Unix socket file, close named pipe handles, delete the port file. The IPC server lifecycle is tied to application exit, NOT to the `stop` playback command — `stop` only halts playback and the engine proceeds through normal shutdown flow.
- **FR-006**: The CLI MUST auto-detect the active transport: try native transport first, then fall back to reading the port file for TCP.
- **FR-007**: The `AROUND_FORCE_TCP` environment variable MUST force TCP transport on all platforms for deterministic testing.
- **FR-008**: Platform-specific transport code MUST be isolated behind `#[cfg(unix)]` (Unix socket) and `#[cfg(windows)]` (named pipe) attributes.
- **FR-009**: The IPC codec MUST support Protobuf as the primary wire format (`ipc-protobuf` feature flag, using `prost`), with JSON Lines retained as a compile-time fallback and runtime debug option. Both codecs MUST implement the same `IpcCodec` trait (async) so transports are format-agnostic.
- **FR-010**: Running-instance detection MUST work correctly for all three transport types: connect-check-before-bind for Unix sockets, `CreateFileW` check for named pipes, port-file connect for TCP.
- **FR-011**: The CLI command `send_ipc_command` MUST try transports in order: native first, then TCP port file. If all fail, it MUST return a "no engine running" error.
- **FR-012**: Integration tests MUST validate Unix socket transport on Linux/macOS, named pipe transport on Windows, and TCP fallback on all platforms via `AROUND_FORCE_TCP`.
- **FR-013**: The Protobuf schema (`.proto` file) MUST define messages for `IpcCommand` and `IpcResponse` with length-delimited framing. The JSON codec MUST be able to parse any message that Protobuf can produce, and vice versa, for forward compatibility during transition.
- **FR-014**: The engine MUST support cross-host communication via TCP and UDP, disabled by default. TCP is enabled via `--ipc-listen-tcp <bind-address>:<port>` or `AROUND_IPC_TCP_BIND`. UDP is enabled via `--ipc-listen-udp <bind-address>:<port>` or `AROUND_IPC_UDP_BIND`. TCP and UDP listeners may be enabled independently and run concurrently alongside platform-native IPC.
- **FR-015**: TCP provides reliable, ordered delivery for streaming and general IPC control. UDP provides connectionless, low-latency delivery intended as the foundation for future real-time features. Each protocol serves its appropriate use case — the sender selects the protocol per-command based on its reliability/latency requirements.
- **FR-016**: The UDP transport MUST use datagram-oriented framing where each UDP packet contains exactly one complete IPC message. Messages exceeding the practical UDP limit (512 bytes safe payload) MUST use TCP instead. The responsibility for selecting the correct protocol lies with the CLI/sender, not the engine.
- **FR-017**: All enabled transports (platform-native, TCP, UDP) MUST run concurrently in separate tokio tasks, sharing the same command dispatch handler. Closing one transport MUST NOT affect others.
- **FR-018**: The IPC server architecture MUST reserve an `IpcConfig` struct with fields: `tcp_bind: Option<SocketAddr>`, `udp_bind: Option<SocketAddr>`, and `enable_platform_native: bool`. Default: platform-native enabled, TCP/UDP disabled. Designed to be populated from CLI/env now and from KDL config in the future. The config struct and transport builder API MUST be public for downstream crate use.
- **FR-019**: The `--remote <host>:<port>` CLI flag MUST direct `send_ipc_command` to bypass local transport discovery and connect directly to the specified remote engine via TCP.
- **FR-020**: Cross-host communication MUST be explicitly opt-in (disabled by default) to comply with Constitution VI (Privacy by Design — zero network by default).

### Key Entities

- **Unix Socket Transport**: A `tokio::net::UnixListener` bound at the platform-standard location (Linux: `$XDG_RUNTIME_DIR/around/`, macOS: `$TMPDIR`, fallback: `/tmp`). Lifecycle: created on `run_ipc_server`, deleted on application exit cleanup.
- **Named Pipe Transport**: A `tokio::net::windows::named_pipe::NamedPipeServer` (tokio `net` feature (already included)). Fully async — no `spawn_blocking` needed. Lifecycle: per-connection instances, stopped on application exit.
- **TCP Fallback Transport**: A `tokio::net::TcpListener` bound at `127.0.0.1:0` for local IPC. When `tcp_bind` is set in IpcConfig, an additional TCP listener binds on the specified remote address. Port discovered via `<temp>/around.port` file for local; remote address is user-configured.
- **Protobuf Codec**: Primary wire format (feature `ipc-protobuf`), using `.proto` schema for `IpcCommand`/`IpcResponse`, length-delimited framing. Implements `IpcCodec` trait (async).
- **JSON Codec**: Fallback/debug wire format, newline-delimited. Implements `IpcCodec` trait (async). Capable of round-tripping with Protobuf for transition compatibility.
- **Transport Discovery**: The mechanism by which the CLI locates the engine — native well-known paths or port file for TCP.
- **UDP Transport**: A `tokio::net::UdpSocket` bound when `udp_bind` is set in IpcConfig. Each datagram is one complete IPC message. Best-effort delivery. Runs concurrently with TCP — may share the same port number (separate protocol stack). Foundation for future low-latency features (real-time control, multi-instance coordination).
- **IpcConfig**: Configuration struct with fields: `tcp_bind: Option<SocketAddr>` (remote TCP listener), `udp_bind: Option<SocketAddr>` (remote UDP listener), `enable_platform_native: bool` (default true). All three transports may be active simultaneously. Public API for downstream crates.
## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: On Linux, `around status` returns a response within 100ms when connecting via Unix socket (same-machine local IPC).
- **SC-002**: On Windows, `around status` returns a response within 100ms when connecting via named pipe.
- **SC-003**: When native transport fails, the engine starts with TCP fallback within 2 seconds of the failure.
- **SC-004**: `cargo test --workspace` passes on all three platforms with all IPC tests exercising the correct transport for that platform.
- **SC-005**: Duplicate engine startup is reliably rejected on all three transport types (zero false-positive "already running" errors, zero false-negative silent overwrites).
- **SC-006**: Transport resource cleanup (socket file, pipe handles, port file) completes within 500ms of engine shutdown on all platforms.
- **SC-007**: When `--ipc-listen-tcp` or `--ipc-listen-udp` is specified, the engine accepts remote connections within 500ms of startup.
- **SC-008**: UDP command processing latency is under 10ms per datagram (single message, no connection overhead).
- **SC-009**: The engine correctly handles all three transport types running simultaneously under load (100 commands/second across all transports) without message loss or corruption.

## Assumptions

- `tokio` provides `UnixListener`/`UnixStream` on Unix platforms — this is the standard async runtime and is already a dependency.
- Windows named pipes are implemented via `tokio::net::windows::named_pipe` (requires tokio `net` feature (already included) on Windows targets). This provides fully async named pipe I/O without raw Win32 FFI or `spawn_blocking`. The `windows-sys` crate is not needed.
- The engine runs as a foreground process that can create multiple IPC endpoints simultaneously (platform-native + TCP + UDP). Each endpoint is independent; failure of one does not affect others.
- The `/tmp` directory exists and is writable on Linux/macOS. Container environments without `/tmp` naturally fall back to TCP.
- The `AROUND_FORCE_TCP` env var is set by tests and CI only; production users never need it.
- The port file (`<temp>/around.port`) is sufficient for TCP discovery — no mDNS or service registry is needed for local IPC.
- The `IpcCodec` trait abstracts wire format from transport. Protobuf (length-delimited binary) is primary; JSON Lines (newline-delimited text) is fallback. Transports are format-agnostic.
