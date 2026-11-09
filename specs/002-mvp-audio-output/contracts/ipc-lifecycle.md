# Contract: IPC Server Lifecycle & Signal Handling

**Version**: 0.1.0
**Feature**: 002-mvp-audio-output
**Extends**: 001-core-playback-engine IPC protocol

The audio and signal sections retain the feature's original requirements;
they are not claims of verified timing or current signal-handler implementation.
The endpoint-resolution and CLI sections describe the current IPC behavior.

## 1. IPC Server Startup

When `cmd_play` is invoked:
1. Create a tokio `current_thread` runtime on a dedicated OS thread.
2. Start `run_ipc_server(IpcConfig, Arc<Engine>)` before audio decode.
3. Make the server accept connections before audio output begins.
4. Keep it alive until playback ends or engine shutdown runs.

## 2. Socket Lifecycle

### 2.1 Endpoint Resolution

The public
`socket_paths(custom_path: Option<&Path>) -> impl Iterator<Item = PathBuf>`
returns the ordered, deduplicated candidates shared by the server, the
duplicate-instance probe, and the CLI:

1. non-empty `$XDG_RUNTIME_DIR/around/around.sock`
2. `std::env::temp_dir()/around.sock`
3. `/tmp/around.sock`

A custom path is the sole candidate. The server tries the candidates in order and
binds the first that succeeds. A live endpoint stops the search with `AddrInUse`.
The path actually bound is logged and used for cleanup; duplicate detection
probes the candidate list before binding.

### 2.2 Fresh Start
- Acquire a nonblocking exclusive lock on `<socket-path>.lock` before probing
  or binding. A held lock means another instance owns or is preparing the
  endpoint; return `AddrInUse` without TCP fallback.
- Bind the resolved endpoint.
- On Unix, set the socket file to `0600` (and a newly created parent directory
  to `0700`).

### 2.3 Stale Unix Socket
- Probe the candidate with `UnixStream::connect`.
- `ConnectionRefused` identifies a stale socket: re-check the path's device and
  inode, then delete it and bind fresh. A different endpoint installed during the
  probe is not removed.
  The lifetime lock serializes competing stale-socket takeovers, including
  when the optional running-instance pre-check is disabled.
- A successful probe means another instance is running. Binding fails with
  `AddrInUse`, and the server MUST NOT fall back to TCP.
- An existing ordinary file or symlink MUST be preserved and reported as
  `AddrInUse`; it is never unlinked as stale.
- Other probe or bind errors leave the candidate untouched; they do not authorize
  unlinking it as stale.
- A candidate that fails with a non-`AddrInUse` error may be followed by the next
  candidate and, finally, the TCP fallback.

### 2.4 Cleanup
On signal, stop, end-of-file, or error:
1. Stop playback.
2. Drop the cpal stream.
3. Stop the IPC runtime and close the endpoint.
4. Delete the endpoint path actually bound.
5. Exit cleanly.

On Unix, the bound socket owns the lock until endpoint cleanup and listener
close complete. Cleanup checks the bound device/inode before removal so it
does not delete a replacement endpoint. The `.lock` file remains on disk:
unlinking it would let contenders acquire locks on different inodes.

### 2.5 CLI Discovery and Error Exits
- CLI control commands resolve the same native candidates first, then read
  `<temp>/around.port` and connect over TCP. `--remote host:port` bypasses local
  discovery and connects directly.
- Any command failure exits with status `1` after printing `error: <message>` to
  stderr. This includes transport/connect failures and server responses whose
  `status` is `error`; a server error is never reported as success.

## 3. Signal Handling

### 3.1 Registered Signals
- SIGINT (Ctrl+C)
- SIGTERM
- SIGHUP

### 3.2 Handler Behavior
Signal handlers MUST only set the shared atomic flag. The main thread performs
cleanup after the decode loop exits.

### 3.3 Timing
- Stop command to audio silence: <500ms.
- Full cleanup, including endpoint deletion: <500ms from signal receipt.
- Process exits after cleanup without waiting for decoder teardown.

## 4. Audio Output Stream

### 4.1 Creation
- After the decoder opens, create an output stream from the negotiated
  `OutputSession`/`SampleSpec`.
- `FilterChain` converts decoded PCM to the bound output format before queuing it.
  The cpal callback only copies converted bytes and fills underruns with
  encoding-correct silence.

### 4.2 Data Flow
- Decoder produces interleaved f32 samples.
- FilterChain applies user filters and terminal conversion.
- OutputSession queues native-format bytes; the cpal callback drains them.

### 4.3 Error Handling
- `StreamError` sets `device_lost = true` and logs a warning.
- With `output_auto_reconnect`, keep trying to recover; otherwise auto-pause.

### 4.4 Cleanup
- Dropping the cpal `Stream` stops audio output.
- Drop it when playback stops or the Engine is dropped.
