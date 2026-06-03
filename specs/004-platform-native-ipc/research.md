# Research: Platform-Native IPC with TCP/UDP Fallback

**Feature**: 004-platform-native-ipc
**Date**: 2026-05-31
**Status**: Complete

## Decision: Protobuf (prost) vs. JSON Wire Format

**Decision**: Protobuf via `prost` as the primary wire format, JSON Lines retained as compile-time fallback and runtime debug option. Both implement the shared `IpcCodec` trait so transports are format-agnostic.

**Rationale**: Protobuf provides compact binary encoding (typically 3–10× smaller than JSON for structured command/response messages), schema evolution through backward-compatible field additions, and a strong Rust ecosystem around `prost`. The `.proto` schema in `contracts/ipc.proto` becomes the authoritative contract for `IpcCommand` and `IpcResponse`. JSON is retained because (1) it enables human debugging via `nc` / `socat` / raw pipe inspection without tooling, (2) it serves as a runtime fallback when protobuf deserialization fails (corrupt message, schema mismatch during transition), and (3) it avoids a hard dependency on `prost-build` for embedded/headless builds that can use `--features ipc-json-only`.

Codec selection is controlled by the `ipc-protobuf` Cargo feature flag:

```
[features]
ipc-protobuf = ["prost", "prost-build"]
```

When the feature is enabled, `ProtoCodec` (length-delimited protobuf framing) is the active codec and JSON is the fallback. When disabled, `JsonLineCodec` (newline-delimited JSON) is the sole codec. Both implement:

```rust
pub(crate) trait IpcCodec {
    async fn read_command<R: AsyncRead + Unpin>(&self, reader: &mut BufReader<R>) -> io::Result<IpcCommand>;
    async fn write_response<W: AsyncWrite + Unpin>(&self, writer: &mut W, resp: &IpcResponse) -> io::Result<()>;
}
```

FR-013 requires JSON↔Protobuf round-trip compatibility during transition: the JSON codec must be able to parse any message Protobuf can produce, and vice versa. This is achieved by keeping `IpcCommand`/`IpcResponse` as the canonical Rust types (with `serde` derives) and generating protobuf ↔ Rust mapping via `prost` that mirrors the same field set.

**Alternatives considered**:
- MessagePack (`rmp-serde`) — compact binary like Protobuf, but schema-less. No `.proto` contract means no compile-time message validation and no schema evolution story beyond "add optional fields." Rejected because the schema *is* the contract between engine and CLI.
- CBOR (`ciborium`) — IETF standard binary encoding. Stronger than MessagePack (canonical representation, self-describing), but Rust ecosystem lags behind `prost`. No code generation. Rejected because Protobuf's `prost-build` codegen eliminates an entire class of hand-written serialization bugs.
- Cap'n Proto (`capnp`) — zero-copy, no encoding/decoding step. Theoretically faster, but build integration is complex (requires `capnp` compiler in `$PATH`, schema plugin, separate codegen pass). Overkill for command-level IPC where a dozen messages per second is the expected workload.
- FlatBuffers — zero-copy like Cap'n Proto, designed for games. Same build complexity problem, and the Rust crate (`flatbuffers`) has less ecosystem traction than `prost`. Rejected as over-engineered for IPC messages that fit in a single packet.

---

## Decision: Platform-Native Transports with TCP Fallback

**Decision**: Use Unix domain sockets on Linux/macOS, named pipes on Windows, with TCP on `127.0.0.1:<random-port>` as universal fallback. All transports implement the same per-connection handler pattern.

**Rationale**: Platform-native transports are faster than TCP loopback because they bypass the full TCP/IP stack — Unix sockets use kernel-level file-descriptor passing with zero-copy, and named pipes use shared memory on Windows. Both respect OS permission models: Unix socket file permissions (0600) prevent other users on the same machine from connecting; named pipes support ACLs. These are the idiomatic local IPC mechanisms on their respective platforms.

TCP is retained as a fallback because it is always available — no filesystem, no special privileges, no named pipe namespace. It also serves as the cross-host transport when `--ipc-listen` is specified, and enables deterministic testing via `AROUND_FORCE_TCP=1`.

The auto-detection order for the CLI is:
1. Try native transport (Unix socket path / named pipe) — fast path, no port file needed
2. Try TCP via port file at `<system-temp-dir>/around.port` — slow path, used when native failed or `AROUND_FORCE_TCP=1`

FR-017 requires all transports to run concurrently in separate tokio tasks, sharing the same `IpcCodec` and command dispatch. This means enabling cross-host TCP does not disable the local Unix socket — both run simultaneously.

**Alternatives considered**:
- TCP-only — simplest implementation (single code path, no `#[cfg]`). Already implemented in the current codebase. Rejected because it surrenders Unix socket permissions (any local user can connect to localhost), adds TCP stack overhead for local communication, and fails Constitution IV (platform-native support). The spec explicitly requires replacing TCP-only IPC.
- HTTP (hyper/warp) — would give us REST semantics and built-in tooling (`curl`), but adds ~20 dependencies, connection-per-request overhead, and forces text-based headers on every message. Overkill for a local command channel handling ~10 msg/s.
- Abstract Unix sockets (Linux `@around`) — namespace-scoped, no filesystem cleanup needed. Rejected because they are Linux-only (no macOS/Windows equivalent) and don't support file-permission-based access control. The filesystem socket path is more portable and debuggable.

---

## Decision: Windows Named Pipes via Raw Win32 API

**Decision**: Use `tokio::net::windows::named_pipe` (`ServerOptions` / `ClientOptions`) — fully async, no raw Win32 FFI needed. The `net` feature we already use includes the necessary `windows-sys` bindings internally.

**Rationale**: tokio 1.x provides built-in async named pipe support via `tokio::net::windows::named_pipe`. This API is stable, idiomatic, and integrates directly with tokio's reactor — no `spawn_blocking`, no unsafe FFI, no extra dependencies. The `net` feature (already enabled in our `Cargo.toml`) includes `windows-sys/Win32_System_Pipes` on Windows targets, so the named pipe types compile automatically.

**Alternatives considered**:
- Raw Win32 FFI via `windows-sys` + `spawn_blocking` — ~100 lines of unsafe I/O code. Rejected because tokio's built-in API provides the same functionality with zero additional code, fully async I/O, and no extra dependency.
- `interprocess` crate — supports local sockets cross-platform but is synchronous-only for named pipes; would still require `spawn_blocking` integration.


## Decision: UDP Transport Foundation

**Decision**: Use `tokio::net::UdpSocket` with datagram-per-message framing. Each UDP packet contains exactly one complete IPC command or response. Best-effort delivery; no retransmission, no ordering guarantees, no acknowledgment.

**Rationale**: UDP provides the lowest-latency IPC transport available on all platforms — no connection handshake, no retransmission state, no head-of-line blocking. It is the right foundation for future real-time features that demand sub-millisecond delivery, broadcast-style messaging, or fire-and-forget command patterns. A single UDP datagram can reach multiple receivers without per-instance connection setup. Processing latency is under 10ms per datagram (SC-008) because there is no TCP handshake, no connection state, no retransmission logic.

The design constraints:
- Each datagram = one complete message, framed by the UDP packet boundary. No length-delimited framing or message reassembly.
- Messages exceeding the practical UDP safe payload (512 bytes) are rejected at the codec level. Commands that would exceed this must use TCP instead (FR-016). This is enforced by the `ProtoCodec` / `JsonLineCodec` returning an error for oversized messages.
- Malformed datagrams are silently logged and ignored — no response is sent (UDP has no connection to respond on) (FR-017 edge case).
- No authentication, no encryption — cross-host UDP is trusted-LAN-only in this phase. A future FR will add authentication.

The UDP transport is enabled via `--ipc-listen-udp` and runs independently of TCP (may share the same port number). When both TCP and UDP are enabled, they run in separate tokio tasks, sharing the same `handle_command` dispatch (FR-017). A TCP client and a UDP datagram arriving simultaneously are handled independently with no race condition on shared state (the `PlaybackState` mutex serializes mutations).

**Alternatives considered**:
- TCP multicast — requires `setsockopt(IP_ADD_MEMBERSHIP)`, adds connection-per-receiver overhead, and doesn't match UDP's fire-and-forget pattern. Rejected because TCP's reliability guarantees (retransmission, ordering) are unnecessary for the command types UDP is best suited for (idempotent, frequent, superseding) — a lost position update is harmless when the next update arrives moments later.
- WebRTC — provides NAT traversal and encrypted transport, but requires ICE/STUN/TURN infrastructure, SDP negotiation, and a signaling channel. Overkill for a trusted LAN where all machines are on the same subnet. Rejected as premature optimization.
- Unix domain datagram sockets (`SOCK_DGRAM`) — connectionless like UDP but local-only. Rejected because a single UDP transport covers both local (`127.0.0.1`) and cross-host use cases, keeping the infrastructure unified for future real-time features.

---

## Decision: IPC Module Extraction

**Decision**: Extract IPC into a sub-module directory `crates/around-engine/src/ipc/` with separate files for each transport, codec, types, and handlers. Replace the single monolithic `ipc.rs`.

**Rationale**: The current `ipc.rs` at ~370 lines handles only TCP with JSON codec. Adding 4 transport implementations (Unix socket, named pipe, TCP, UDP), 2 codecs (Protobuf, JSON), shared types, handlers, and config would push a single file past 800 lines — well beyond the maintainability threshold for a module with this many orthogonal concerns. The proposed structure:

```
ipc/
├── mod.rs              # Transport selection, IpcConfig, run_ipc_server orchestration
├── transport_unix.rs   # #[cfg(unix)] Unix domain socket listener + accept loop
├── transport_win.rs    # #[cfg(windows)] Named pipe server via Win32 API
├── transport_tcp.rs    # TCP listener (fallback + cross-host), cross-platform
├── transport_udp.rs    # UDP socket (foundation for future real-time features), cross-platform
├── codec.rs            # IpcCodec trait, JsonLineCodec, ProtoCodec
├── types.rs            # PlaybackState, IpcCommand, IpcResponse (moved from ipc_types.rs)
├── handlers.rs         # handle_command and per-command dispatch (moved from ipc.rs)
└── discovery.rs        # Transport auto-detection for CLI (well-known paths, port file)
```

This follows Rust convention (e.g., `tokio::net` splits `tcp.rs`, `unix.rs`, `udp.rs`) and makes each file independently understandable: a developer debugging a Windows named pipe issue opens `transport_win.rs` and sees ~100 lines of Win32 FFI without scrolling past Unix socket or Protobuf code. Each transport file exposes a single `async fn run_*_server(...)` entry point called from `mod.rs`.

The `#[cfg]` gating lives at the module level: `transport_unix.rs` is `#[cfg(unix)]`, `transport_win.rs` is `#[cfg(windows)]`. `mod.rs` uses conditional compilation to select the appropriate transport:

```rust
#[cfg(unix)]
mod transport_unix;
#[cfg(windows)]
mod transport_win;
mod transport_tcp;
mod transport_udp;
```

This replaces the current `ipc.rs` / `ipc_codec.rs` / `ipc_types.rs` trio, which grew organically and lacks clear separation.

**Alternatives considered**:
- Single `ipc.rs` — simplest for one transport, but adding 3 more transports + 1 more codec + config + discovery would create an ~800-line file with `#[cfg]` branches interleaved throughout. Rejected on maintainability grounds: the complexity table entry in plan.md already justifies this as a necessary complexity increase.
- Separate crate (`around-ipc`) — would isolate IPC completely, but requires publishing types (`IpcCommand`, `IpcResponse`, `PlaybackState`) in a shared crate to avoid circular dependencies with `around-engine`. Rejected because (1) the types are deeply coupled to the engine's state model, (2) no other crate consumes IPC types directly, and (3) the IPC module is not independently reusable — it exists to serve `around-engine`'s control plane. Module-level separation is the right granularity.
