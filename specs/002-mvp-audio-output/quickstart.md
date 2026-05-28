# Quickstart: MVP Audio Output Pipeline & Critical Fixes

## Build

```bash
cargo build --release
```

## Run

### Play audio with IPC control (two terminals)

**Terminal 1** (playback):
```bash
./target/release/around play examples/long-test.wav
```

**Terminal 2** (control):
```bash
./target/release/around status
./target/release/around pause
./target/release/around resume
./target/release/around seek --position 30
./target/release/around stop
```

### Signal handling

```bash
# Ctrl+C (SIGINT) works as stop
./target/release/around play examples/example.wav
# Press Ctrl+C

# SIGTERM
kill <pid>

# SIGHUP
kill -HUP <pid>
```

All three signals produce the same clean shutdown: audio stops, socket is deleted, process exits.

## Run Tests

```bash
# Full suite
cargo test

# IPC lifecycle tests specifically
cargo test -- test ipc_control

# With logging
RUST_LOG=around_engine=debug cargo test -- --nocapture
```

## Verify IPC Lifecycle

```bash
# 1. Start playback
./target/release/around play examples/long-test.wav &

# 2. Socket should exist
ls -la /tmp/around.sock

# 3. Status should work
./target/release/around status

# 4. Stop
./target/release/around stop

# 5. Socket should be gone
ls /tmp/around.sock  # should print "No such file or directory"

# 6. Second instance detection
./target/release/around play examples/long-test.wav &
./target/release/around play examples/long-test.wav  # should error
```
