# Research: MVP Audio Output Pipeline & Critical Fixes

**Feature**: 002-mvp-audio-output
**Date**: 2026-05-28
**Status**: Complete

## Decision: cpal Output Stream Integration

**Decision**: Create a dedicated OS thread for the cpal output stream. The decode loop feeds PCM samples into a crossbeam channel; the cpal callback reads from the channel.

**Rationale**: cpal's `build_output_stream` requires a callback that runs on the audio thread with real-time constraints. The decode loop (which may block on I/O) cannot run inside the callback. A channel-based approach decouples the two:

1. Decode thread fills a ring buffer (PCM samples)
2. cpal callback reads from the ring buffer on each frame request
3. When the channel is empty (buffer underrun), the callback outputs silence until more data arrives

**Alternatives considered**:
- Blocking read in callback — violates cpal's real-time requirement; audio glitches on I/O wait.
- Lock-free MPSC queue — ideal but more complex to implement correctly. Crossbeam channel is sufficient for MVP.

## Decision: SIGHUP/SIGTERM Signal Handling

**Decision**: Use tokio's `signal` module to handle SIGTERM alongside the existing ctrlc handler for SIGINT. For SIGHUP, also register via tokio signal.

**Rationale**: The existing ctrlc crate handles only SIGINT. Tokio's signal module supports SIGTERM and SIGHUP natively on Unix. Since we already have a tokio runtime for IPC, adding signal watchers is natural.

```rust
use tokio::signal::unix::{signal, SignalKind};

let mut term = signal(SignalKind::terminate())?;
let mut hup = signal(SignalKind::hangup())?;
```

Signals set the same `running` atomic flag as Ctrl+C, triggering the same cleanup path.

**Alternatives considered**:
- `signal_hook` crate — well-established but adds another dependency. Tokio already in the dependency tree.
- Raw `libc` signal handlers — unsafe and error-prone; async-signal-safe restrictions limit what handlers can do.

## Decision: Static Dispatch Restoration

**Decision**: Restore the concrete `WavDecoder` path in `Engine::play()` for the direct WAV play case. Keep `try_open_decoder` for fallback scenarios only.

**Rationale**: The hot decode loop is the performance-critical path. Restoring concrete type dispatch eliminates vtable overhead per `read()` call. The fallback chain (extension decoders) can use `Box<dyn Decoder>` since it's invoked only on the first call, not per-frame.

## Decision: IPC Server in cmd_play

**Decision**: Spawn `run_ipc_server` on a separate thread (using tokio::runtime::Runtime) within `cmd_play`, before starting the decode loop. The IPC server runs until playback completes or a signal is received.

**Rationale**: `run_ipc_server` is async (tokio-based). `cmd_play` blocks on the decode loop. Solution: create a tokio `current_thread` runtime in a separate thread for the IPC server. When playback ends, drop the runtime (cleanly shuts down the IPC listener and closes the socket).

```rust
let ipc_thread = thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .unwrap();
    rt.block_on(run_ipc_server("/tmp/around.sock", engine))
});
```

## Decision: Device Disconnect Detection

**Decision**: cpal's `StreamError` callback sets `Engine::device_lost` flag. The main decode loop checks this flag and auto-pauses. IPC status returns `device_lost: bool`.

**Rationale**: cpal's `build_output_stream` accepts an error callback. This is the natural hook for device loss detection.
