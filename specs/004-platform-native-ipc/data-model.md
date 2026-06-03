# Data Model: Platform-Native IPC with TCP Fallback

**Feature**: 004-platform-native-ipc
**Date**: 2026-05-31

## Entity Overview

```text
┌──────────────┐     ┌─────────────────┐     ┌──────────────┐
│   IpcConfig  │────▶│ TransportState  │────▶│  IpcCommand  │
└──────────────┘     └─────────────────┘     └──────────────┘
       │                      │                      │
       ▼                      ▼                      ▼
  bind_address          per-transport         dispatched to
  proto selection       lifecycle             command handler
  cross_host flag
                                                     │
                            ┌──────────────┐          │
                            │  IpcCodec    │◀─────────┘
                            └──────────────┘
                                   │
                                   ▼
                            ┌──────────────┐
                            │ IpcResponse  │
                            └──────────────┘

Transport layer:   ┌──────────┐  ┌───────────┐  ┌──────────┐  ┌──────────┐
                   │UnixSocket│  │ NamedPipe │  │   TCP    │  │   UDP    │
                   └──────────┘  └───────────┘  └──────────┘  └──────────┘
                   #[cfg(unix)]  #[cfg(windows)]  cross-plat   cross-plat
```

All four transport types run concurrently in separate tokio tasks, sharing a single
command dispatch. Each transport manages its own `TransportState`. The `IpcCodec`
trait decouples wire format from transport — transports are format-agnostic.

## Entity Definitions

### IpcConfig

Reserved configuration struct for future KDL config integration. Currently
populated from CLI flags and environment variables. Public API for downstream
crate use.

```rust
#[derive(Debug, Clone)]
pub struct IpcConfig {
    /// When set, binds a TCP listener on this address for remote control.
    /// `None` means no remote TCP (local-only platform-native IPC + TCP fallback on 127.0.0.1).
    pub tcp_bind: Option<SocketAddr>,

    /// When set, binds a UDP socket on this address for low-latency remote control.
    /// `None` means no remote UDP. May be set independently of `tcp_bind`.
    pub udp_bind: Option<SocketAddr>,

    /// Whether platform-native IPC (Unix socket / named pipe) is enabled. Default true.
    pub enable_platform_native: bool,
}

impl Default for IpcConfig {
    fn default() -> Self {
        Self {
            tcp_bind: None,
            udp_bind: None,
            enable_platform_native: true,
        }
    }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| tcp_bind | `Option<SocketAddr>` | `None` | Remote TCP listener address. `None` = no remote TCP. |
| udp_bind | `Option<SocketAddr>` | `None` | Remote UDP listener address. `None` = no remote UDP. |
| enable_platform_native | `bool` | `true` | Start platform-native IPC (Unix socket or named pipe). |

**Validation**:
- `tcp_bind` or `udp_bind` port, when set, MUST be > 1024 (unprivileged range).
- `tcp_bind` or `udp_bind` MUST be a valid `SocketAddr`.
- TCP and UDP may share the same port number (e.g., `--ipc-listen-tcp 0.0.0.0:9123 --ipc-listen-udp 0.0.0.0:9123`) because they are separate protocol stacks in the OS network layer. The user configures each independently.
- All remote communication is opt-in: if neither `tcp_bind` nor `udp_bind` is set, the engine binds only platform-native IPC (and TCP fallback on `127.0.0.1:0` for local discovery).

**Construction path**: CLI flags (`--ipc-listen-tcp`, `--ipc-listen-udp`) and env vars (`AROUND_IPC_TCP_BIND`, `AROUND_IPC_UDP_BIND`) populate `tcp_bind` and `udp_bind` via `IpcConfig::builder()`. Future: KDL config merges into the same struct.

**State transitions**: Immutable after construction. A new `IpcConfig` is built for each engine start.

**Why TCP and UDP are independent fields (not an enum)**: TCP and UDP serve fundamentally different use cases. TCP provides reliable streaming for content delivery and general control; UDP provides low-latency best-effort messaging for real-time sync. Both can — and often should — run concurrently. A mutually-exclusive enum would force the user to choose one protocol when they actually need both. The spec (FR-014, FR-015) explicitly requires independent enablement per use case.

### TransportState

### TransportState

Per-transport lifecycle state machine. Each transport (Unix socket, named pipe,
TCP, UDP) tracks its own state independently. Transitions are driven by the
transport's tokio task and the shared shutdown signal.

```text
                        ┌──────────────┐
                        │   Stopped    │◀────────────────────────────┐
                        └──────┬───────┘                             │
                               │                                     │
                          bind()                                     │
                               │                                     │
                        ┌──────▼───────┐    BindFailed               │
                        │   Binding    │──────────────┐              │
                        └──────┬───────┘              │              │
                               │                      │              │
                          bind succeeds               │              │
                               │                      │              │
                        ┌──────▼───────┐              │              │
                        │  Listening   │              │              │
                        └──────┬───────┘              │              │
                               │                      │              │
                          shutdown()                  │              │
                               │                      │              │
                        ┌──────▼───────┐              │              │
                        │ ShuttingDown │              │              │
                        └──────┬───────┘              │              │
                               │                      │              │
                          cleanup()                   │              │
                               │                      │              │
                               └──────────────────────┼──────────────┘
                                                      │
                                          cleanup() on error
```

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportState {
    /// Initial state. No resources allocated.
    Stopped,
    /// bind() called; attempting to acquire OS resource (socket, pipe, port).
    Binding,
    /// Successfully bound and accepting connections / datagrams.
    Listening,
    /// shutdown() received; draining existing connections, no new accepts.
    ShuttingDown,
    /// bind() failed. Terminal error state — triggers fallback logic.
    BindFailed,
}
```

| State | Meaning | Allowed Transitions |
|-------|---------|---------------------|
| `Stopped` | Initial and terminal. No resources held. | → `Binding` via `bind()` |
| `Binding` | OS resource acquisition in progress. | → `Listening` on success; → `BindFailed` on error |
| `Listening` | Accepting connections / datagrams. | → `ShuttingDown` via `shutdown()` |
| `ShuttingDown` | Draining, no new accepts. | → `Stopped` via `cleanup()` |
| `BindFailed` | Terminal error, triggers fallback. | → `Stopped` via `cleanup()` (releases partial resources) |

**Transition semantics**:

- **`bind()`**: Acquires the OS resource. For Unix sockets: create + bind +
  listen. For named pipes: `tokio::net::windows::named_pipe::ServerOptions`. For TCP/UDP: `TcpListener::bind`
  / `UdpSocket::bind`. On failure, transitions to `BindFailed`. Must be
  idempotent — calling `bind()` on a non-`Stopped` state is a no-op (logged).

- **`shutdown()`**: Signals the transport to stop accepting new connections.
  Existing connections are allowed to drain (complete their current
  request-response cycle). For UDP (connectionless), this stops the recv loop
  immediately. Must be idempotent — calling `shutdown()` on an already-stopping
  transport is a no-op.

- **`cleanup()`**: Releases all OS resources. For Unix sockets: `close()` +
  `unlink()` the socket file. For named pipes: `DisconnectNamedPipe` +
  `CloseHandle`. For TCP: close listener. For UDP: close socket. Deletes the
  port file (`<temp>/around.port`) if this transport wrote it. Idempotent. On
  completion, always transitions to `Stopped`.

**Fallback logic**: When a transport enters `BindFailed`, the transport manager
(see `transport selection` in the architecture) attempts the next transport in
priority order: Unix socket → named pipe → TCP fallback. Only the TCP fallback
has no further fallback — if TCP on `127.0.0.1:0` fails, the engine starts
without IPC and logs a fatal warning (audio playback still works).

**Concurrency**: State transitions are guarded by a `tokio::sync::RwLock`.
Transitions are write-locked; status queries (e.g., `is_listening()`) are
read-locked. Transitions themselves are fast (atomic flag changes + OS calls
already made), so lock contention is negligible.

---

### IpcCodec Trait

Abstracts wire format from transport. Every transport reads/writes through this
trait — transports do not know whether they carry Protobuf or JSON. Async
variant for all tokio transports (Unix socket, TCP, UDP, named pipe).
#[async_trait]
pub trait IpcCodec: Send + Sync {
    /// Read and decode one command from the transport reader.
    async fn read_command<R: AsyncRead + Unpin + Send>(
        &self,
        reader: &mut BufReader<R>,
    ) -> io::Result<IpcCommand>;

    /// Encode and write one response to the transport writer.
    async fn write_response<W: AsyncWrite + Unpin + Send>(
        &self,
        writer: &mut W,
        resp: &IpcResponse,
    ) -> io::Result<()>;
}

/// Sync codec for blocking I/O transports (Windows named pipes).
pub trait IpcCodecSync {
    /// Read and decode one command from a blocking reader.
    fn read_command_sync<R: Read + BufRead>(
        &self,
        reader: &mut R,
    ) -> io::Result<IpcCommand>;

    /// Encode and write one response to a blocking writer.
    fn write_response_sync<W: Write>(
        &self,
        writer: &mut W,
        resp: &IpcResponse,
    ) -> io::Result<()>;
}
```

**Implementations**:

| Codec | Trait Impl | Framing | Feature Gate |
|-------|-----------|---------|--------------|
| `JsonLineCodec` | `IpcCodec` + `IpcCodecSync` | Newline-delimited (`\n` terminator) | Always compiled |
| `ProtoCodec` | `IpcCodec` + `IpcCodecSync` | Length-delimited (varint prefix) | `ipc-protobuf` feature |

**JsonLineCodec** (existing, extracted to trait impl):
- Each message is one line of JSON terminated by `\n`.
- Empty lines are skipped (read resilience).
- Encoding: `serde_json::to_string` + `\n`.
- Always available — no feature gate, no build dependency.

**ProtoCodec** (new):
- Each message is prefixed with a Protobuf varint length, then the encoded
  protobuf bytes.
- Decoding: read varint → read N bytes → `prost::Message::decode`.
- Encoding: `prost::Message::encode` → write varint length → write bytes.
- Feature-gated behind `ipc-protobuf` for builds where binary size matters
  (embedded, headless).

**Codec selection**: At compile time, if `ipc-protobuf` is enabled, `ProtoCodec`
is the default and `JsonLineCodec` is the fallback. At runtime, if Protobuf
decoding fails for a message, the codec layer transparently attempts JSON
decoding before returning an error. This enables graceful transition and
debugging.

**Validation**: Both codecs MUST be able to round-trip all valid `IpcCommand`
and `IpcResponse` values. The Protobuf schema (see `contracts/ipc.proto`) is
the canonical definition; the JSON codec is validated against it in tests.

---

### IpcCommand

Incoming IPC command, received from clients and dispatched to the engine's
command handler. All variants carry their arguments inline (no separate payload
protocol). Tagged by the `command` field in JSON; Protobuf uses `oneof`.

```rust
#[derive(Debug, Deserialize)]
#[serde(tag = "command")]
pub enum IpcCommand {
    /// Start or restart playback of a file.
    Play { path: String },

    /// Pause playback at current position. No-op if already paused/stopped.
    Pause,

    /// Resume playback from paused position. No-op if playing or stopped.
    Resume,

    /// Seek to an absolute position in milliseconds.
    Seek { position_ms: u64 },

    /// Stop playback and unload the current track.
    Stop,

    /// Query current playback state. Returns `IpcResponse` with state fields populated.
    Status,

    /// Query available audio decoders. Returns `decoders` in response.
    ListDecoders,

    /// Load a decoder library from the filesystem path.
    LoadDecoder { path: String },

    /// Load a decoder library from in-memory bytes (base64-encoded in the `data` field).
    LoadDecoderBytes { data: String, name: String },

    /// Trigger engine cleanup: flush pipelines, release resources, remove stale files.
    Cleanup,
}
```

| Variant | Arguments | Idempotent | Side Effects |
|---------|-----------|------------|--------------|
| `Play` | `path: String` | Yes (restarts if already playing) | Loads track, starts audio output |
| `Pause` | — | Yes | Pauses audio stream |
| `Resume` | — | Yes | Resumes audio stream |
| `Seek` | `position_ms: u64` | Yes | Repositions playback cursor |
| `Stop` | — | Yes | Stops audio, unloads track. Does NOT shutdown IPC server. |
| `Status` | — | Yes | None (read-only) |
| `ListDecoders` | — | Yes | None (read-only) |
| `LoadDecoder` | `path: String` | No (second load is error) | Registers new codec |
| `LoadDecoderBytes` | `data: String, name: String` | No (second load is error) | Registers new codec from bytes |
| `Cleanup` | — | Yes | Releases resources, removes stale files |

**Key invariant**: `Stop` only stops playback. It does **not** shut down the IPC
server. The IPC server lifecycle is tied to application exit, per specification
clarification. This means `Stop` → `Status` returns `state: "stopped"` with the
server still reachable.

**Source**: Defined in `crates/around-engine/src/ipc_types.rs`. This file is
always compiled (no feature gate) since both engine and CLI depend on it.

---

### IpcResponse

Outgoing IPC response, returned by the command handler to the client. All fields
are optional except `status`, which is always `"ok"` or `"error"`.

```rust
#[derive(Debug, Serialize)]
pub struct IpcResponse {
    /// Always present: "ok" or "error".
    pub status: String,

    /// Playback state string ("playing", "paused", "stopped"). Present for Status command.
    pub state: Option<String>,

    /// Current playback position in milliseconds. Present for Status command.
    pub position_ms: Option<u64>,

    /// Error code when status is "error". Machine-readable, stable across versions.
    pub code: Option<String>,

    /// Human-readable error message when status is "error".
    pub message: Option<String>,

    /// Track identifier (numeric). Present for Status command when a track is loaded.
    pub track_id: Option<u64>,

    /// Full track metadata as a JSON object. Present for Status command.
    pub track: Option<serde_json::Value>,

    /// List of available decoders (each a JSON object). Present for ListDecoders command.
    pub decoders: Option<Vec<serde_json::Value>>,

    /// Flag: the audio device was lost since last Status query. Requires engine restart.
    pub device_lost: Option<bool>,

    /// Files removed by the Cleanup command.
    pub removed_files: Option<Vec<String>>,
}
```

| Field | Type | Present When | Description |
|-------|------|-------------|-------------|
| `status` | `String` | Always | `"ok"` or `"error"` |
| `state` | `Option<String>` | Status command | `"playing"`, `"paused"`, `"stopped"` |
| `position_ms` | `Option<u64>` | Status command | Milliseconds since track start |
| `code` | `Option<String>` | Error responses | Machine-readable error code |
| `message` | `Option<String>` | Error responses | Human-readable error description |
| `track_id` | `Option<u64>` | Status command | Loaded track identifier |
| `track` | `Option<Value>` | Status command | Track metadata object |
| `decoders` | `Option<Vec<Value>>` | ListDecoders | Available decoder list |
| `device_lost` | `Option<bool>` | Status command | Audio device lost flag |
| `removed_files` | `Option<Vec<String>>` | Cleanup command | Files removed during cleanup |

**Serialization**: Uses `skip_serializing_if = "Option::is_none"` for all
optional fields — absent fields are omitted from JSON output. Protobuf
encoding uses `optional` fields for the same semantics.

**Factory methods**:
- `IpcResponse::ok()` — returns `{ "status": "ok" }` with all optional fields
  `None`. Callers populate additional fields as needed.
- `IpcResponse::error(code, msg)` — returns `{ "status": "error", "code": ...,
  "message": ... }`.

**Source**: Defined in `crates/around-engine/src/ipc_types.rs`, re-exported
from `around-engine` for CLI use.

---

### PlaybackState

Internal engine state, updated by command handlers and read when constructing
`IpcResponse` for `Status` commands. Not transmitted directly — it is the
engine's truthful view, mapped into `IpcResponse` fields.

```rust
#[derive(Debug, Clone)]
pub struct PlaybackState {
    pub playing: bool,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub track_path: Option<String>,
    pub format_name: Option<String>,
    pub output_format: Option<SampleSpec>,
    pub seekable: bool,
    pub device_lost: bool,
    pub state: String,  // "playing" | "paused" | "stopped"
}
```

**State mapping to IpcResponse**:

| PlaybackState field | IpcResponse field | Notes |
|---------------------|-------------------|-------|
| `state` | `state` | Direct copy |
| `position_ms` | `position_ms` | Direct copy |
| `track_path` + metadata | `track` | Assembled into a JSON object |
| `device_lost` | `device_lost` | Direct copy |

**Default**: `state: "stopped"`, all boolean fields `false`, all numeric fields
`0`/`None`.

**Source**: Defined in `crates/around-engine/src/ipc_types.rs`.

---

## Cross-Entity Relationships

```text
IpcConfig ────────────── determines which transports start ──▶ TransportState[]
     │                                                              │
     │ tcp_bind + udp_bind: TCP/UDP independence                                        │
     │ tcp_bind / udp_bind: Option<SocketAddr>                             │
     │                                                              │
     └──────────────────────────────────────────────────────────────┘
                                                                    │
                                           each transport uses ─────┘
                                                                    │
                                                                    ▼
                                                            IpcCodec impl
                                                                    │
                                                    ┌───────────────┴───────────────┐
                                                    │                               │
                                               read_command()              write_response()
                                                    │                               │
                                                    ▼                               ▼
                                              IpcCommand                      IpcResponse
                                                    │                               ▲
                                                    │                               │
                                                    └─────── dispatched to ─────────┘
                                                            command handler
                                                                    │
                                                                    ▼
                                                            PlaybackState
                                                          (engine internal)
```

**Cardinality**:
- One `IpcConfig` per engine process (1:1).
- One `TransportState` per transport instance (up to 4: Unix + named pipe + TCP + UDP).
- One `IpcCodec` impl active per connection (per-message selection; all share the same trait).
- One `PlaybackState` shared across all transports via `Arc<RwLock<PlaybackState>>`.

**Sharing**: All transport tasks share the same `Arc<Engine>` (for command
dispatch) and `Arc<RwLock<PlaybackState>>` (for status queries). The `IpcConfig`
is read-only after construction and shared via `Arc<IpcConfig>`.

**Shutdown ordering**: On application exit:
1. `shutdown()` is called on all transports concurrently.
2. Each transport transitions `Listening → ShuttingDown`, drains connections, then calls `cleanup()`.
3. Once all transports reach `Stopped`, the IPC subsystem signals completion.
4. The engine then drops the `Engine` (which stops audio output if running).

## Validation Rules

| Rule | Scope | Enforcement |
|------|-------|-------------|
| Port > 1024 | `IpcConfig::bind_address` | Parse-time, `IpcConfig::builder()` |
| Valid `SocketAddr` | `IpcConfig::bind_address` | Parse-time, `std::net::SocketAddr` parsing |
| Non-loopback + `enable_cross_host: false` | `IpcConfig` | Builder error |
| `Both` + port+1 > 65535 | `IpcConfig` | Builder error |
| UDP payload ≤ 512 B | `ProtoCodec` / `JsonLineCodec` | Encode-time rejection |
| Empty command is skipped | `JsonLineCodec::read_command` | Runtime, empty-line skip |
| Protobuf decode failure → JSON fallback | Codec layer | Runtime, transparent |
| `bind()` on non-`Stopped` transport | `TransportState` | No-op with trace log |
| `--remote` + no engine running | CLI discovery | Error: "no engine running" |

## Future Extensibility

- **IpcConfig**: Fields added via new optional members. KDL config file merges
  into the same builder API. No breaking change to existing callers using
  `Default::default()`.
- **TCP/UDP independence**: Additional protocols (e.g., WebSocket for browser clients)
  can be added as new enum variants. `Both` is already forward-compatible with
  `All([TCP/UDP independence; N])` if more than two protocols are needed.
- **IpcCodec**: Additional codecs (MessagePack, CBOR) implement the same trait.
  Transports are unaffected.
- **TransportState**: Additional transient states (e.g., `Reconnecting` for TCP
  keep-alive) can be inserted without breaking the existing transition
  contracts, since state transitions are encapsulated within each transport
  module.
- **IpcCommand / IpcResponse**: New variants/fields are additive. The Protobuf
  `oneof` and `optional` semantics ensure backward compatibility. The JSON codec
  ignores unknown fields via `serde_json` defaults.
