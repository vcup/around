# Quickstart: Platform-Native IPC Development

## Prerequisites

- Rust stable (1.85+), pinned via `rust-toolchain.toml`
- Platform audio libraries (same as core engine):
  - Linux: `libasound2-dev` (ALSA) or `libpulse-dev` (PulseAudio)
  - macOS: Xcode Command Line Tools (CoreAudio built-in)
  - Windows: WASAPI (built-in)
- Protobuf compiler (`protoc`) — only needed when building with `ipc-protobuf` feature

## Build

```bash
# Full workspace build — compiles all transports:
#   Linux/macOS: Unix domain sockets + TCP/UDP
#   Windows:     named pipes + TCP/UDP
cargo build --workspace
```

All platform-specific transport code is `#[cfg]`-gated, so the compiled binary only
contains the transports relevant to the host OS. TCP and UDP are cross-platform.

## Run Tests

### Deterministic cross-platform tests (TCP fallback)

```bash
# Forces TCP transport on all platforms for test determinism.
# Works identically on Linux, macOS, and Windows — no native transport required.
AROUND_FORCE_TCP=1 cargo test --workspace
```

### Native transport tests (platform-specific)

```bash
# Runs IPC integration tests exercising the platform's native transport:
#   Linux:   Unix domain socket at $XDG_RUNTIME_DIR/around/around.sock
#   macOS:   Unix domain socket at $TMPDIR/around.sock
#   Windows: named pipe at \\.\pipe\around
cargo test --test ipc_control
```

### With logging

```bash
# Trace-level IPC diagnostics — inspect transport selection, codec handshakes,
# per-connection spans, and command dispatch.
RUST_LOG=around_engine=trace cargo test -- --nocapture
```

## Protobuf Codec

The `ipc-protobuf` feature (off by default) enables the binary protobuf wire format
via `prost`. Without it, the engine uses JSON Lines for all IPC.

```bash
# Build with protobuf support
cargo build --features ipc-protobuf

# Run tests with protobuf codec active
AROUND_FORCE_TCP=1 cargo test --features ipc-protobuf --workspace
```

When `ipc-protobuf` is enabled, the engine negotiates protobuf first and falls back
to JSON if the client does not support it.

## Cross-Host Remote Control

Cross-host communication is **disabled by default** (Constitution VI — zero network).
Enable it explicitly with CLI flags or environment variables.

### Start the engine with cross-host enabled

```bash
# Terminal 1 — bind TCP on all interfaces, port 19123.
# The local Unix socket (Linux/macOS) or named pipe (Windows) still runs.
./target/debug/around play examples/long-test.wav \
  --ipc-listen-tcp 127.0.0.1:19123
```

### Connect from a remote client

```bash
# Terminal 2 (same machine, or another machine on the LAN)
./target/debug/around --remote 127.0.0.1:19123 status
```

### Run integration tests against a remote engine

```bash
# With the engine already running on 127.0.0.1:19123 (see above):
cargo test --test ipc_control -- --remote 127.0.0.1:19123
```

### UDP transport (low-latency, connectionless)

```bash
# Start engine with UDP
./target/debug/around play examples/long-test.wav \
  --ipc-listen-udp 0.0.0.0:19123
```

UDP is a foundation for future low-latency features (e.g. multi-device clock
sync). Each UDP datagram carries one complete IPC message.

### Environment variable equivalents

```bash
# Start engine with TCP remote listener
AROUND_IPC_TCP_BIND=0.0.0.0:9123 \
  ./target/debug/around play examples/long-test.wav
```

## Verify IPC Lifecycle

```bash
# 1. Start playback (background)
./target/debug/around play examples/long-test.wav &

# 2. Socket should exist (Linux/macOS)
ls -la /tmp/around.sock

# 3. Status via auto-detected transport
./target/debug/around status

# 4. Pause / resume
./target/debug/around pause
./target/debug/around resume

# 5. Seek
./target/debug/around seek --position 30

# 6. Stop (playback stops; engine shuts down; socket is cleaned up)
./target/debug/around stop

# 7. Socket should be gone
ls /tmp/around.sock   # "No such file or directory"

# 8. Duplicate instance detection
./target/debug/around play examples/long-test.wav &
./target/debug/around play examples/long-test.wav   # should error: engine already running
```

## Environment Variables

| Variable | Values | Purpose |
|---|---|---|
| `AROUND_FORCE_TCP` | `1` | Force TCP transport on all platforms (test determinism) |
| `AROUND_IPC_TCP_BIND` | `<addr>:<port>` | Enable TCP cross-host listener |
| `AROUND_IPC_UDP_BIND` | `<addr>:<port>` | Enable UDP cross-host listener |
| `RUST_LOG` | `around_engine=trace` (or `debug`, `info`) | Control tracing verbosity for IPC diagnostics |

`AROUND_FORCE_TCP` is intended for testing and CI only — production users do not
need it. The engine auto-selects the native transport by default.

## CI

GitHub Actions tests all three platforms on every push and PR to `master`:

| Job | OS | Transports Tested |
|---|---|---|
| `linux` | ubuntu-latest | Unix sockets (+ TCP fallback) |
| `macos` | macos-latest | Unix sockets (+ TCP fallback) |
| `windows` | windows-latest | Named pipes (+ TCP fallback) |

Workflow: `.github/workflows/buid-tests.yaml`

Each job runs `cargo build --workspace` and `cargo test --workspace`. Audio tests
skip automatically when no output device is available (CI VMs). IPC tests exercise
the platform's native transport — Unix sockets on Linux/macOS, named pipes on Windows.
