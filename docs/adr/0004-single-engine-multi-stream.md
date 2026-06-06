# ADR 0004: Single-Engine Multi-Stream Architecture

The Engine manages 0..N concurrent playback Streams within a single process.
Each Stream is an independent decode loop feeding a lock-free ring buffer
consumed by the CPAL output callback. The Engine is the routing matrix — not
a single-playback session.

## Status

Accepted

## Context

Two architectural paths were evaluated:

1. **Multi-Engine**: One `Engine` struct per playback session. Each Engine owns
   its own state, decoder, and output binding. Shared resources (Registry,
   ExtensionManager) live in `Arc`s above the Engine layer.

2. **Single-Engine Multi-Stream**: One `Engine` struct manages N concurrent
   `Stream`s. Each Stream is a decode loop + ring buffer + output binding. The
   Engine is the routing matrix.

### Evaluation

| | Multi-Engine | Single-Engine Multi-Stream |
|---|---|---|
| Stream isolation | Natural (separate structs) | Requires per-stream cancel tokens |
| Registry sharing | Arc (trivial) | Natural (Engine owns it) |
| Routing matrix | Needs separate Router component above Engines | Engine IS the router |
| Cross-device sync | Distributed clock needed | Single clock source |
| Memory | Duplicated module list per Engine | One Registry instance |
| Constitution alignment | Violates routing matrix requirement | Satisfies all directives |

The constitution (spec `004-platform-native-ipc`) requires: "any source MAY be
routed to any combination of output sinks." The routing matrix is the Engine's
identity, not an add-on.

### Industry precedent

| System | Model | Relevance |
|---|---|---|
| MPD | Single-process, single-stream | Simple but cannot route |
| VLC/libVLC | Both modes supported | Multi-instance preferred for isolation |
| PipeWire | Graph daemon, unlimited streams | Best alignment: one process, per-stream routing |
| Voicemeeter | Single-process matrix | M × N routing in one process |
| OBS | Single-process mixer | Multiple sources → multiple outputs |

PipeWire and Voicemeeter demonstrate that single-process multi-stream routing
is a mature, proven pattern.

## Decision

**Single `Engine` manages N `Stream`s.** Each Stream is created by a `Play`
IPC command, assigned a monotonically incrementing `StreamId`, and runs an
asynchronous decode loop on the Engine's tokio runtime. The decode loop writes
PCM samples into a lock-free single-producer-single-consumer ring buffer
(`rt-ring` crate). The CPAL output callback reads from the ring buffer on its
own high-priority real-time thread.

### Stream lifecycle

```
Play → create Stream → async decode loop → ring buffer → CPAL callback
                                                  ↑
                                          FilterChain (inline, future)
Stop/Cancel/Error → cancel token → decode loop exits → stream removed
```

### Per-stream cancel

Each Stream holds a `CancellationToken` created via
`Engine.shutdown_token.child_token()`. Engine shutdown broadcasts cancel to
all streams with zero allocation and zero iteration.

### State

`StreamState` uses all-Atomic fields — no `Mutex`:

- `status: Atomic<TrackState>` via `atomic-rs` (`Atomic<T>` generic wrapper)
- `position_ms: AtomicU64` — hot path, single writer
- `device_lost: AtomicBool` — CPAL callback writes
- `active: AtomicBool` — CPAL callback reads, decode loop sets after pre-fill

### Device loss policy (future)

Device reconnection behavior is configurable per device type:

- Bluetooth headset → auto-reconnect + auto-continue
- USB DAC → auto-reconnect + pause (don't auto-continue)
- Onboard speaker → stop on loss (don't reconnect)

The policy lives in Engine config. StreamState only tracks the `device_lost`
flag. In the 004 branch, all devices default to auto-reconnect.

### FilterChain (future)

Superseded by ADR-0005.

### Router (future)

The Router fans out a single Stream's ring buffer to N consumers
(physical devices, virtual loopback, network sinks, file recorders).
It is a 1‑to‑N SPSC bridge: a tokio task reads from the Stream's
ring buffer and writes to each consumer's independent SPSC ring.
Every consumer ring has its own Resample filter if its target format
differs from the Stream's `output_spec`.

When there is exactly one consumer, the Router is absent — the Stream
ring buffer connects directly to the consumer.

### Forwarding (future)

Default strategy per connection type:

| Mode | Data | Bandwidth | Use case |
|---|---|---|---|
| Decoded PCM | Stream ring buffer samples | High | LAN, low latency |
| Source bytes | Encoded Source output | Low | WAN, cross‑device |

Alternative: send synchronised timestamps only; the receiving instance
pulls the source independently (network sources download directly;
local files are synced to peers). All modes are user‑configurable.

### Source trait

The `Source` trait is asynchronous. All I/O (disk, network) is `async`.
The decode loop awaits `source.read(&mut buf).await?` on the main
tokio runtime. This enables io_uring on Linux and natural composability
with network streams — no `spawn_blocking` for I/O, only for CPU work
(decode + filter).
## Consequences

- `PlaybackState` struct replaced by `StreamState` with Atomic fields — no
  lock contention between decode loop and IPC handler
- `epoch` counter eliminated — new `play` awaits old loop exit via cancel
  token before starting new loop
- `Engine.running: Arc<AtomicBool>` replaced by `CancellationToken` tree
- Per-stream state isolation eliminates the need for global mutexes
  (`CLEANUP_MUTEX`, `ENV_MUTEX`) in integration tests
- `load_bytes` `name` parameter eliminated — temp filename generated from
  content hash (separate ADR)
