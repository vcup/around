# 004-platform-native-ipc — Consolidated Audit

**Generated**: 2026-06-02  
**Source**: Cross-referenced findings from 5 per-code-block subagents + 12 prior audits.  
**Status**: Some findings from prior audits already fixed (see "Previously Fixed" appendix).

---

## 🔴 Bugs — MUST Fix

### B1. `cmd_serve --background` exits immediately, killing daemon (CLI:C1)

**File**: `crates/around-cli/src/main.rs`  
**Severity**: Critical  

`std::thread::spawn` creates a non-detached thread. When `cmd_serve` returns `Ok(())` after spawning, `main()` reaches end of match arm and the process exits. The spawned tokio runtime thread is killed by the OS. Engine never runs.

```rust
if background {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()...;
        let _ = rt.block_on(around_engine::ipc::run_ipc_server(ipc_config, eng, st));
    });
    return Ok(());  // process exits, kills thread
}
```

**Fix options**: (a) Join the thread (block foreground), (b) `std::process::Command` to re-spawn detached child, (c) Unix: fork+setsid. See decision request A1 below.

---

### B2. `cmd_shutdown` sends `"stop"` not engine-shutdown signal (CLI:C2)

**File**: `crates/around-cli/src/main.rs`  
**Severity**: Critical  

```rust
fn cmd_shutdown(remote: Option<&str>) -> ... {
    let req = serde_json::json!({"command": "stop"});
```

`"stop"` triggers `handle_stop` which stops playback but does **not** terminate the engine process. The engine's `run_ipc_server` polls `engine.is_shutdown()` — there is no IPC command that signals shutdown. The `Shutdown` subcommand is a no-op.

**Fix**: Either add a `handle_shutdown` handler that calls `engine.signal_shutdown()`, or add a `"shutdown"` command to the existing handler dispatch that triggers the shutdown signal.

---

### B3. `cmd_seek` negative f64 → u64 undefined behavior (CLI:C3)

**File**: `crates/around-cli/src/main.rs`  
**Severity**: Critical  

```rust
let position_ms = (position * 1000.0) as u64;
```

`position: f64` from CLI `--position` flag. Negative input (`around seek --position -5.0`) triggers UB: the `as u64` cast on a negative float is undefined behavior per Rust reference.

**Fix**: Validate before cast:
```rust
if position < 0.0 {
    return Err("position must be non-negative".into());
}
```

---

### B4. Race: old playback loop cleanup corrupts new playback state (Handlers:C1)

**File(s)**: `crates/around-engine/src/ipc/handlers.rs:13-76`, `crates/around-engine/src/pipeline.rs:431-436`  
**Severity**: Critical  

`engine.stop()` sets `self.running.store(false)` but does **not** synchronize with the exiting `engine.play()` loop. The old loop's cleanup block unconditionally writes `state="stopped"` after the new handler has already set up `state="buffering"` for the new track.

**Race timeline**:
1. `handle_play` calls `engine.stop()` → `running = false`  
2. Handler sets `state = "buffering"`, `playing = true`  
3. Handler spawns new `tokio::spawn(…play(…))`  
4. **Old** play loop exits while, locks state, writes `state = "stopped"` ← corrupts  
5. New play loop eventually sets `state = "playing"`  

Between steps 4-5, Status returns "stopped" despite new playback in flight. If step 5 fails (codec open error), the "stopped" state is permanent.

**Fix**: Options:
- (A) Hold the state lock across `engine.stop()` + state setup + spawn — prevents old loop from acquiring lock during transition
- (B) Playback loop checks a generation counter before writing cleanup state
- (C) `engine.stop()` synchronously joins the playback thread

---

### B5. Path traversal in `handle_load_decoder_bytes` (Handlers:C2)

**File**: `crates/around-engine/src/ipc/handlers.rs:198-218`  
**Severity**: Critical  

The `name` field from IPC is passed directly into filesystem path construction with no sanitization:

```rust
let tmp_path = tmp_dir.join(format!("around_decoder_{}.{}", name, DLL_EXTENSION));
```

`name = "../../../.ssh/authorized_keys"` escapes `tmp_dir`. `name = "/etc/cron.d/evil"` (absolute) replaces the base path entirely.

**Fix**: Sanitize at handler entry:
```rust
if name.contains('/') || name.contains('\\') || name.contains("..") || name.is_empty() {
    return IpcResponse::error("INVALID_NAME", "decoder name must not contain path separators");
}
```

---

## 🟠 Design Issues — SHOULD Fix

### D1. `handle_play` response lies about state (Handlers:H3)

**File**: `handlers.rs:58-74`  
**Severity**: High  

Handler writes `st.state = "buffering"` (correct) but response claims `state: "playing"`. The actual playback hasn't started — it's inside `tokio::spawn` + `spawn_blocking`. The response should match the shared state the handler just wrote.

**Fix**: Set `resp.state = Some("buffering".into())`.

---

### D2. `handle_status` missing `track_id` (Handlers:H2)

**File**: `handlers.rs:138-155`  
**Severity**: High  

`data-model.md` says Status command must return `track_id`. `handle_status` never sets it. `handle_play` sets `resp.track_id = Some(1)` in its response, but Status — the canonical query — omits it.

**Fix**: Add `resp.track_id = Some(1)` (or derive from track identity) in `handle_status`.

---

### D3. `cmd_load_decoder`/`list_decoders`/`cleanup` exit 0 on engine error (CLI:H4)

**File**: `main.rs`  
**Severity**: High  

These three commands check `resp["status"] == "ok"`, print to stderr, but always `return Ok(())`. Scripts relying on exit codes get false positives — the command reports success but the operation failed.

**Fix**: Propagate error:
```rust
if resp["status"] != "ok" {
    return Err(resp["message"].as_str().unwrap_or("unknown error").into());
}
```

---

### D4. Windows named pipe: `reject_remote_clients(false)` violates Constitution VI (Transports:C2)

**File**: `transport_win.rs`  
**Severity**: High  

`reject_remote_clients(false)` allows remote pipe connections. Constitution VI requires cross-host functionality disabled by default. A remote Windows user could connect to the pipe if network sharing is enabled.

**Fix**: Set `reject_remote_clients(true)` unless `config.remote_tcp_bind` is explicitly configured.

---

### D5. `bind_unix_socket` vs `bind_unix_socket_at` — 85% code duplication + divergent error handling (Transports:Dup)

**File**: `transport_unix.rs:16-50`  
**Severity**: Medium  

The two functions share ~85% identical code but diverge in error handling: `bind_unix_socket` uses `tracing::warn!` for permission failures, `bind_unix_socket_at` silently swallows via `let _ = ...`.

**Fix**: Merge into one function with a path parameter.

---

### D6. `handle_cleanup` matches `around-decoder-` pattern that never exists (Handlers:H4)

**File**: `handlers.rs:157-185`  
**Severity**: Medium  

Cleanup looks for both `around-decoder-` (hyphens) and `around_decoder_` (underscores). `handle_load_decoder_bytes` only creates files with underscore prefix. The hyphen branch is dead matching logic.

**Fix**: Remove `around-decoder-` branch or document as legacy naming.

---

## 🟡 Code Quality — SHOULD Fix

### Q1. `_map_engine_error` dead function (Handlers:H1)

**File**: `handlers.rs:260-273`  

Function never called. Underscore prefix suppresses only the warning, not the dead code emission. Delete it.

---

### Q2. `read_line` unbounded — OOM DoS vector (Codec:M1)

**File**: `codec.rs:44-46`  

No max line length. Client sending stream without `\n` causes unbounded growth. Local-only by default but TCP exposes to LAN.

**Fix**: Check `line.len() > MAX_LINE_LEN` (e.g., 64 KiB) after each `read_line` and return error.

---

### Q3. `line.clear()` dead code — allocations not reused (Codec:L1)

**File**: `codec.rs:44-52`  

`line.clear()` inside loop body, but `line` is re-created each iteration via `let mut line = String::new()`. The cleared string is dropped immediately. Hoist `let mut line` before the loop; `clear()` becomes meaningful.

---

### Q4. `handle_seek` no bounds check on `position_ms` (Handlers:M7)

**File**: `handlers.rs:107-124`  

Seek past `duration_ms` causes playback loop to hit EOF silently. `position_ms` values causing `target_ms * sample_rate` overflow in pipeline cause wrap/panic in debug.

**Fix**: Validate against `st.duration_ms` and add overflow guard.

---

### Q5. `IpcResponse.output_format` write-only field (Types:H1)

**File**: `types.rs`  

`output_format` is set by the pipeline but never included in IPC responses. It exists in the data model but is never exposed to clients. Either expose it or remove the field.

---

## 🔵 Needs Architecture Decision

### A1. macOS `$TMPDIR` sandbox mismatch (Transports:C1)

macOS `$TMPDIR` is per-process and differs between sandboxed and non-sandboxed processes. If the engine runs sandboxed and the CLI runs from Terminal, they resolve different `$TMPDIR` values — socket communication fails entirely.

**Question**: Is the engine expected to run sandboxed on macOS? Should the socket path be a well-known fixed location (e.g., `$HOME/Library/Caches/around/around.sock`) instead of `$TMPDIR`?

---

### A2. `cmd_serve` missing `--ipc-listen-tcp/udp` flags (CLI:H2)

`cmd_serve` hardcodes `IpcConfig::default()`. Unlike `cmd_play`, `Serve` has no flags for cross-host remote control. If a user wants a persistent engine accessible over TCP, they must use `play` (which exits after playback) rather than `serve`.

**Question**: Should `serve` have the same `--ipc-listen-tcp` and `--ipc-listen-udp` flags as `play`?

---

### A3. `cmd_play --remote` semantic (CLI:H3)

`cmd_play` ignores `--remote` and always builds a local engine. FR-019 implies `--remote` should delegate to a remote engine. But `play` with a local file path when the engine is remote raises a design question: should `play --remote host song.wav` send the file path to the remote engine (which needs filesystem access to the same path)?

**Question**: 
- (a) `play --remote host file.wav` should delegate to remote (file must exist on remote host)
- (b) `play --remote` should error: "play requires local engine, use serve+play separately"
- (c) `play --remote host file.wav` should upload the file to remote then play

---

### A4. CLI transport connect/read timeouts (CLI:M1)

All three transport connection paths use blocking `std::net` with no timeout:
- `TcpStream::connect(addr)?` — blocks until OS-level TCP timeout (minutes)
- `UnixStream::connect(&path)?` — blocks until kernel responds
- Named pipe `OpenOptions::new().open(...)` — blocks until server calls `ConnectNamedPipe`

If the engine is hung or the socket path is wrong, CLI hangs forever.

**Question**: Accept this as a known limitation of `std::net` (mitigated by near-instant local IPC), or introduce `socket2`/tokio for timeout support?

---

### A5. Windows named pipe ACL for multi-user (Transports:C3)

Windows named pipes have no access control — any local user can connect and control playback. On single-user desktops this is fine; on multi-user servers or shared machines, it's a risk.

**Question**: Add ACL restricting pipe to creating user? Defer until multi-user is a supported scenario?

---

## Test Gaps

### T1. No CLI tests at all

`crates/around-cli/tests/` does not exist. All CLI behavior is untested: exit codes, error messages, argument parsing edge cases, transport auto-detection order.

### T2. `cmd_shutdown` integration test

Spawn engine, send shutdown, verify process exits within timeout, verify socket/port file cleaned up.

### T3. `cmd_seek` negative position rejection

Verify `around seek --position -1.0` returns non-zero exit and error message.

### T4. Race condition stress test for B4

Rapid-fire Play → Play → Status in tight loop; assert Status never returns "stopped" while new track is loading.

### T5. Path traversal rejection for B5

Send `LoadDecoderBytes` with `name = "../../../etc/passwd"`; assert error response with code `INVALID_NAME`.

### T6. handle_play response state consistency for D1

Issue Play, immediately Status. Both must agree on state field.

### T7. handle_seek overflow/past-duration for Q4

Seek to `u64::MAX` on loaded track — must not panic. Seek past duration — must return `INVALID_POSITION`.

---

## Previously Fixed

These findings from prior audits have been resolved in the current implementation:

| Prior ID | Description | Resolution |
|----------|-------------|------------|
| C1 (ref) | Windows `first_pipe_instance(true)` blocks concurrent | Removed in 91cff25 |
| C2 (ref) | Per-transport `ExtensionManager` inconsistent state | Shared in `run_ipc_server` |
| C5 (cons) | TCP-only running-instance check not gated | Removed via unified TCP flow |
| C6 (cons) | ENV_MUTEX missing from one test | Added |
| H1-H2 (cons) | No CLI flag for native toggle | Config-based, not CLI flag |
| H4 (ref) | Shared port file race | Unique paths per test |
| FR-004/5/6/10/11/12/14/19/37 (spec) | Various spec gaps | All fixed in spec.md |
| C1 (unix) | TOCTOU on socket permissions | Accepted window; set_permissions errors now logged |
| C1 (fallback) | `!enable_platform_native` control-flow fork | Removed entirely |
