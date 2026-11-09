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
  bind addresses        lifecycle             command handler
  wire-format choice
                                                     │
                            ┌──────────────┐          │
                            │   IpcWire    │◀─────────┘
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

All configured transports run concurrently in separate tokio tasks and share
one command dispatcher and `Arc<Engine>`. `IpcWire` selects the wire format per
connection. `TransportState` below is the design-level lifecycle model; the
current implementation tracks spawned transport tasks in `TransportManager`,
and each Unix task captures its bound path for cleanup.

## Entity Definitions

### IpcConfig

Configuration for one engine process. CLI flags and environment variables
populate it; KDL integration is future work. It is immutable after construction
and public for downstream crates.

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

    /// Custom port file path for TCP fallback discovery.
    /// `None` means `<temp>/around.port`.
    pub port_file_path: Option<PathBuf>,

    /// Check for a running engine instance before binding.
    /// Tests may disable the pre-check to exercise bind-time detection.
    pub check_running_instance: bool,

    /// Custom Unix socket path. `None` resolves platform candidates.
    pub unix_socket_path: Option<PathBuf>,

    /// Force JSON wire format even when protobuf is compiled. For tests.
    pub force_json: bool,
}

impl Default for IpcConfig {
    fn default() -> Self {
        Self {
            tcp_bind: None,
            udp_bind: None,
            enable_platform_native: true,
            port_file_path: None,
            check_running_instance: true,
            unix_socket_path: None,
            force_json: false,
        }
    }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| tcp_bind | `Option<SocketAddr>` | `None` | Remote TCP listener address. `None` = no remote TCP. |
| udp_bind | `Option<SocketAddr>` | `None` | Remote UDP listener address. `None` = no remote UDP. |
| enable_platform_native | `bool` | `true` | Start platform-native IPC (Unix socket or named pipe). |
| port_file_path | `Option<PathBuf>` | `None` | TCP fallback discovery file; `None` = `<temp>/around.port`. |
| check_running_instance | `bool` | `true` | Probe for a live engine before binding. |
| unix_socket_path | `Option<PathBuf>` | `None` | Single custom Unix endpoint; `None` = platform candidates. |
| force_json | `bool` | `false` | Select JSON even when protobuf is compiled. |

**Validation**:
- `tcp_bind` and `udp_bind` MUST parse as `SocketAddr`; `from_parts` /
  `from_cli_env` reject malformed values.
- Ports `> 1024` are a feature requirement but are not enforced by `IpcConfig`
  today.
- TCP and UDP may share a port number because they use separate protocol stacks.
- All remote communication is opt-in. With neither remote bind set, the engine
  uses platform-native IPC and starts the loopback TCP fallback only if native
  binding fails (a live native endpoint is a hard error, not a fallback trigger).

**Construction path**: CLI flags (`--ipc-listen-tcp`, `--ipc-listen-udp`) and
env vars (`AROUND_IPC_TCP_BIND`, `AROUND_IPC_UDP_BIND`) populate `tcp_bind` and
`udp_bind` via `IpcConfig::from_cli_env` (or `from_parts` in tests); CLI values
take precedence over env values. Future: KDL config merges into the same struct.

TCP and UDP run concurrently and remain separate: TCP provides reliable
delivery; UDP provides low-latency, best-effort delivery.

### Socket Paths

`socket_paths(custom_path: Option<&Path>) -> impl Iterator<Item = PathBuf>` is the
single ordered, deduplicated resolver shared by the server, the duplicate-instance
probe, and the CLI. Its default candidates are:

1. non-empty `$XDG_RUNTIME_DIR/around/around.sock`
2. `std::env::temp_dir()/around.sock`
3. `/tmp/around.sock`

A custom path is the sole candidate. Binding rules:

- Before probing or binding, acquire a nonblocking exclusive `flock` on
  `<socket-path>.lock`. A held lock is `AddrInUse`, including while the winning
  process is still preparing its listener. Hold it through listening and cleanup.
  Keep the lock file across restarts: deleting it could let contenders lock
  different inodes for the same endpoint.
- A successful probe means a live instance: return `AddrInUse` and do not fall
  back to TCP.
- A probe rejected with `ConnectionRefused` identifies a stale socket, which may
  be removed and rebound after re-checking its device/inode.
- An existing ordinary file or symlink MUST be preserved and reported as
  `AddrInUse` (no fallback).
- Other errors leave the candidate untouched; default binding tries the next
  candidate and returns the last error if none bind.
- Cleanup removes the path actually bound only while its device/inode still
  matches the owned socket, then closes the listener and releases the lock.

**CLI discovery**: `--remote host:port` connects directly over TCP. Otherwise
`send_ipc_command` tries the native candidates, then `<temp>/around.port` over
TCP. Every failure propagates to `main`, which prints `error: ...` to stderr and
exits with status `1`.

### TransportState

Each transport's task owns its listener and bound path. The intended
per-transport lifecycle is:

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
    /// Bind failed; fallback logic may select another transport.
    BindFailed,
}
```

No public `TransportState` enum is exported today; this is the design-level
state machine. The current `TransportManager` owns the spawned task handles and
the TCP port-file path; each Unix task captures its actual bound path for
cleanup.

| State | Meaning | Allowed transitions |
|-------|---------|---------------------|
| `Stopped` | Initial and terminal; no resources held. | → `Binding` via `bind()` |
| `Binding` | Acquiring the OS resource. | → `Listening` on success; → `BindFailed` on error |
| `Listening` | Accepting connections or datagrams. | → `ShuttingDown` via `shutdown()` |
| `ShuttingDown` | Draining; no new accepts. | → `Stopped` via `cleanup()` |
| `BindFailed` | Binding failed; fallback may run. | → `Stopped` via `cleanup()` |

**Design-target transition semantics**:
- `bind()` acquires the OS resource and is a no-op outside `Stopped`.
- `shutdown()` stops new accepts; existing stream connections drain, while UDP
  stops its receive loop immediately. Repeated calls are no-ops.
- `cleanup()` releases OS resources, removes Unix socket and port files, and
  ends in `Stopped`. It is idempotent.

**Fallback**: After native binding fails with any error other than `AddrInUse`,
the manager tries loopback TCP. A live native endpoint (`AddrInUse`) is a hard
error. If TCP also fails, playback continues without IPC and the host logs the
error.

**Concurrency**: The runtime does not share a `TransportState` lock. Each
transport task owns its listener, and `TransportManager` stores the join handles
and bound paths. Engine status is read from per-stream atomics (ADR-0005), not
from transport state.

---

### IpcCodec and IpcWire (ADR-0003)

`IpcCodec` is a `pub(crate)` transport-agnostic async trait. It has no `Send`
supertrait, and its generic reader/writer bounds do not require `Send`; concrete
transports and the `IpcWire` enum are `Send`/`Clone`.

```rust
pub(crate) trait IpcCodec {
    /// Read and decode one command from the transport reader.
    async fn read_command<R: AsyncRead + Unpin>(
        &self,
        reader: &mut BufReader<R>,
    ) -> io::Result<IpcCommand>;

    /// Encode and write one response to the transport writer.
    async fn write_response<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        resp: &IpcResponse,
    ) -> io::Result<()>;
}
```

ADR-0003 selects implementations through a concrete enum rather than
`Box<dyn IpcCodec>`:

```rust
#[derive(Clone)]
pub(crate) enum IpcWire {
    #[cfg(feature = "protobuf-ipc")]
    Proto(ProtoCodec),
    #[cfg(feature = "json-ipc")]
    Json(JsonLineCodec),
}
```

| Codec | Framing | Feature |
|-------|---------|---------|
| `JsonLineCodec` | Newline-delimited (`\n`) | `json-ipc` |
| `ProtoCodec` | Varint length-delimited | `protobuf-ipc` |

`IpcWire::select(force_json)` uses protobuf when compiled unless `force_json`;
if only one codec feature is enabled, that codec is used. A connection selects
JSON only when its first non-whitespace byte is `{`; every other prefix uses the
listener's configured codec. There is no per-message protobuf-to-JSON retry.
`JsonLineCodec` skips up to 16 consecutive empty lines and rejects lines over
64 KiB; `ProtoCodec` rejects messages over 64 MiB. Both codecs MUST round-trip
every valid `IpcCommand` and `IpcResponse`.

---

### IpcCommand

Incoming IPC command, dispatched to the Engine. JSON uses the `command` tag;
Protobuf uses `oneof`.

```rust
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum IpcCommand {
    Play {
        path: String,
        #[serde(default)]
        stream_id: Option<u64>,
    },
    Pause {
        #[serde(default)]
        stream_id: Option<u64>,
    },
    Resume {
        #[serde(default)]
        stream_id: Option<u64>,
    },
    Seek {
        position_ms: u64,
        #[serde(default)]
        stream_id: Option<u64>,
    },
    Stop {
        #[serde(default)]
        stream_id: Option<u64>,
    },
    Status {
        #[serde(default)]
        stream_id: Option<u64>,
    },
    ListCodecs,
    LoadCodec {
        path: String,
    },
    LoadCodecBytes {
        /// Base64 string in JSON; raw bytes in Protobuf.
        #[serde(deserialize_with = "deser_base64_bytes")]
        data: Vec<u8>,
    },
    Cleanup,
    Shutdown,
}
```

| Variant | Arguments | Idempotent | Side Effects |
|---------|-----------|------------|--------------|
| `Play` | `path: String`, optional `stream_id` | No; creates a new stream | Starts playback and returns the allocated `stream_id`. The supplied `stream_id` is currently ignored. |
| `Pause` / `Resume` | optional `stream_id` | Yes | Pauses or resumes the resolved stream |
| `Stop` | optional `stream_id` | Yes | Stops one stream; omitting `stream_id` stops all streams |
| `Seek` | `position_ms: u64`, optional `stream_id` | Yes | Repositions the resolved stream |
| `Status` | optional `stream_id` | Yes | Read-only; omitting `stream_id` returns the `streams` array plus sole-stream aliases |
| `ListCodecs` | — | Yes | Read-only |
| `LoadCodec` | `path: String` | No; duplicate load errors | Registers a codec |
| `LoadCodecBytes` | `data: Vec<u8>` (base64 text in JSON) | No; duplicate load errors | Registers a codec from memory |
| `Cleanup` | — | Yes | Removes stale temporary files |
| `Shutdown` | — | Yes | Calls `Engine::shutdown` |

For `Pause`, `Resume`, and `Seek`, an omitted `stream_id` resolves the sole active
stream and returns `NoTrack` otherwise. `Status` without an id returns all streams
(including an empty array when idle). `Stop` halts playback but
leaves the IPC server reachable. `Shutdown` cancels the engine shutdown token and
all stream tokens; the handler does not terminate the process itself — process
exit is the host's responsibility.

**Source**: `crates/around-engine/src/ipc/types.rs`, always compiled and
re-exported for CLI use.

---

### IpcResponse

Outgoing response. `status` is always `ok` or `error`; optional fields are
omitted from JSON when absent.

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct IpcResponse {
    pub status: ResponseStatus,
    pub state: Option<PlaybackStatus>,
    pub position_ms: Option<u64>,
    pub code: Option<ErrorCode>,
    pub message: Option<String>,
    pub track_id: Option<u64>,
    pub track: Option<TrackInfo>,
    pub codecs: Option<Vec<CodecDescriptor>>,
    pub device_lost: Option<bool>,
    pub removed_files: Option<Vec<String>>,
    pub stream_id: Option<u64>,
    pub content_type: Option<String>,
    pub seekable: Option<bool>,
    pub output_spec: Option<SampleSpec>,
    pub streams: Option<Vec<StreamStatus>>,
}
```

`IpcResponse::ok()` initializes an empty success response.
`IpcResponse::error(code, message)` initializes a machine-readable error.

**Source**: `crates/around-engine/src/ipc/types.rs`, re-exported by
`around-engine` for CLI use.

---

### StreamState and ActiveStream (ADR-0005)

The pre-ADR-0005 single-stream `PlaybackState` struct and its
`Arc<Mutex<PlaybackState>>` / `Arc<RwLock<PlaybackState>>` sharing are
superseded and are not present in the current engine. One `Engine` owns 0..N
streams:

```rust
pub struct Engine {
    // config omitted
    device_lost: Arc<AtomicBool>,
    streams: RwLock<HashMap<StreamId, ActiveStream>>,
    shutdown_token: CancellationToken,
}

pub struct StreamState {
    pub active: AtomicBool,
    pub position_ms: AtomicU64,
    status: AtomicPlaybackStatus,
    pub device_lost: AtomicBool,
}
```

`PlaybackStatus` is the `around_core` enum
`Playing | Paused | Stopped | Buffering | Error`. `status` is private behind
atomic `status()` / `set_status()` accessors. The decode loop writes
`position_ms`; device-loss handling updates `device_lost`. `active` is set when
the stream is prepared and cleared when its decode loop ends.

`ActiveStream` stores the stream id, cancellation token, `Arc<StreamState>`,
source/codec metadata, duration, seekable flag, and output spec. `Engine` takes
the `parking_lot::RwLock` only for topology changes and lookups; hot status
fields stay atomic. `handle_status` snapshots each stream into `StreamStatus` and
fills the sole-stream alias fields when exactly one stream is active.

**Source**: `crates/around-engine/src/pipeline.rs`.

---

## Cross-Entity Relationships

```text
IpcConfig ────────────── determines which transports start ──▶ transport tasks
     │                                                              │
     │ tcp_bind / udp_bind: Option<SocketAddr>                      │
     │ enable_platform_native / unix_socket_path                    │
     │                                                              │
     └──────────────────────────────────────────────────────────────┘
                                                                    │
                                           each transport uses ─────┘
                                                                    │
                                                                    ▼
                                                                IpcWire
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
                                                    Engine → StreamState[] (atomics)
                                                            (engine internal)
```

**Cardinality**:
- One `IpcConfig` per engine process (1:1).
- One transport task per enabled transport (up to 3: Unix **or** named pipe, TCP, UDP). `TransportState` is the design model; the runtime stores task handles.
- One `IpcWire` per listener, cloned per connection; JSON auto-detection may override it for that connection.
- One `Engine` per process, owning 0..N `StreamState`s through `ActiveStream` entries; all transports share `Arc<Engine>`.

**Sharing**: All transport tasks share the same `Arc<Engine>` for command
dispatch and status snapshots. Hot status fields come from per-stream atomics
(ADR-0005); metadata and format lookup go through the `Engine` topology map.
There is no shared `Arc<RwLock<PlaybackState>>`. `TransportManager` owns its
`IpcConfig` by value; spawned tasks capture the settings they need.

**Shutdown ordering**: On application exit:
1. `Engine::shutdown()` cancels the shutdown token and every stream token.
2. Accept loops observe `Engine::is_shutdown()`; a loop blocked in `accept()`
   must return before it observes cancellation.
3. Each transport task removes the endpoint path it created, then
   `TransportManager::join` completes.
4. The host drops the `Engine` and the audio output stream.

## Validation Rules

| Rule | Scope | Enforcement |
|------|-------|-------------|
| Valid `SocketAddr` | `IpcConfig` | Parse-time rejection |
| Port `> 1024` | `IpcConfig` | Documented requirement; not enforced today |
| UDP datagram > 65507 B | UDP transport | Dropped with warning |
| UDP datagram > 512 B | UDP transport | Accepted with fragmentation warning |
| More than 16 consecutive empty lines | `JsonLineCodec` | Error |
| JSON line > 64 KiB / Protobuf message > 64 MiB | Codec | Error |
| Live Unix endpoint | Unix bind | `AddrInUse`; no TCP fallback |
| Ordinary file or symlink at endpoint | Unix bind | Preserved; `AddrInUse`; no TCP fallback |
| All CLI discovery transports fail | CLI | `error: ...` on stderr, exit `1` |

## Future Extensibility

- **IpcConfig**: Add new optional fields; preserve `Default::default()` callers.
  KDL configuration will merge into the same construction path.
- **IpcCodec**: New wire formats implement the same trait and add an `IpcWire`
  variant; transports are unaffected.
- **TransportState**: New transient states can be added inside transport
  modules without changing existing callers.
- **IpcCommand / IpcResponse**: Additive fields and variants preserve Protobuf
  `oneof`/`optional` and JSON unknown-field behavior.
