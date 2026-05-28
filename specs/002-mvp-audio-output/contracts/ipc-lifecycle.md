# Contract: IPC Server Lifecycle & Signal Handling

**Version**: 0.1.0
**Feature**: 002-mvp-audio-output
**Extends**: 001-core-playback-engine IPC protocol

## 1. IPC Server Startup

When `cmd_play` is invoked:

1. Create a tokio `current_thread` runtime on a dedicated OS thread
2. Before starting audio decode, call `run_ipc_server("/tmp/around.sock", engine)`
3. The server must be accepting connections before audio output begins
4. The server must remain alive until playback ends (file complete, stop, or signal)

## 2. Socket Lifecycle

### 2.1 Fresh Start (no socket file)
- Bind at `/tmp/around.sock`
- Set permissions to 0600 after bind

### 2.2 Stale Socket (socket exists but no live server)
- Attempt `UnixStream::connect`
- If connection refused: delete file, bind fresh
- If connection succeeds: another instance is running → error and exit

### 2.3 Running-Instance Detection
- On successful connect: return error `"another engine instance is already running on /tmp/around.sock"`
- Do NOT overwrite the socket

### 2.4 Cleanup
On any exit (signal, stop, file complete, error):
- Delete `/tmp/around.sock`
- The tokio runtime drop handles listener cleanup

## 3. Signal Handling

### 3.1 Registered Signals
- SIGINT (Ctrl+C) — via ctrlc crate
- SIGTERM — via tokio::signal::unix
- SIGHUP — via tokio::signal::unix

### 3.2 Handler Behavior (must be async-signal-safe)
The handler MUST only set an atomic boolean flag (`running = false`).
All cleanup MUST happen on the main thread after the decode loop exits.

### 3.3 Cleanup Sequence (executed on main thread)
1. Set `running = false` (decode loop exits)
2. Drop the audio output stream (cpal stream drops automatically)
3. Drop the tokio runtime (IPC listener stops, socket closes)
4. Delete `/tmp/around.sock`
5. Process exits

### 3.4 Timing
- Stop command to audio silence: <500ms
- Full cleanup (including socket deletion): <500ms from signal receipt
- Process exit: immediate after cleanup (no timeout waiting for decoder teardown)

## 4. Audio Output Stream

### 4.1 Creation
- After decoder opens successfully, create an audio output stream matching the decoded format's sample rate and channel count.
- If the device cannot match the format exactly, the platform audio subsystem handles conversion.
&nbsp;

### 4.2 Data Flow
- Decoder produces interleaved f32 samples (as per Decoder trait contract)
- Samples are written to a ring buffer (crossbeam channel)
- cpal callback reads from the ring buffer and copies to output buffer

### 4.3 Error Handling
- `StreamError` from cpal callback: set `device_lost = true`, log warning, continue
- If `output_auto_reconnect` config is true: keep trying to play
- If `output_auto_reconnect` config is false: auto-pause playback

### 4.4 Cleanup
- Dropping the cpal `Stream` object stops audio output
- Drop happens when tokio runtime is dropped or when Engine is dropped
