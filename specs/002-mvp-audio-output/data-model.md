# Data Model: MVP Audio Output Pipeline & Critical Fixes

**Feature**: 002-mvp-audio-output
**Date**: 2026-05-28

## Entity Overview

No new entities are introduced. This feature connects the decoder, the
pre-existing output pipeline, and the IPC lifecycle.

```text
Decoder → FilterChain → OutputSession → cpal Stream
    │            │             │              │
    │            │             │              └─ device_lost
    │            │             └─ native-format ring buffer
    │            └─ terminal format conversion
    └─ decoded interleaved f32 PCM

Engine running/shutdown state ──shared with── IPC server
```

## Playback and IPC Lifecycle

| State | Description | IPC endpoint |
|---|---|---|
| `Stopped` | No playback; endpoint may remain alive until engine exit | Listening or absent |
| `Starting` | Binding and initializing before audio starts | Created with platform permissions |
| `Playing` | Audio output and IPC are active | Accepting |
| `Draining` | Playback ended; resources are being released | Closing |
| `Stopped (clean)` | Resources released | Absent after engine exit |

`Stop` and end-of-file end playback, not the engine's IPC service. In foreground
`play`, the host subsequently requests engine shutdown; `serve` remains alive
until shutdown is requested. Transport loops check the shutdown flag and remove
their endpoint when they exit; blocked I/O must return before that check.
The original audio-device recovery requirements are retained in `spec.md`;
they are distinct from this IPC lifecycle.

## Validation Rules

- Native IPC endpoints use their platform-specific permission rules; Unix socket
  files are owner-only (`0600`).
- The default Unix endpoint is the first bindable candidate from
  `$XDG_RUNTIME_DIR/around/around.sock`, `<temp>/around.sock`, and
  `/tmp/around.sock` (unique and ordered). An explicit custom path is the sole
  candidate.
- At startup, a probe that connects means another instance: binding fails with
  `AddrInUse` and does not fall back to TCP. Only a probe rejected with
  `ConnectionRefused` marks a stale socket; removal re-checks the path's
  device/inode so a different endpoint installed during the probe is preserved.
  Ordinary files and symlinks are preserved and reported as `AddrInUse`. Cleanup
  removes the path actually bound, not the requested candidate.
- CLI commands resolve the same native candidates, then use
  `<temp>/around.port` TCP discovery unless `--remote` is supplied. Transport
  failures and server error responses exit `1` with `error: ...` on stderr.
- `OutputSession` queues bytes matching the bound `SampleSpec`; the cpal
  callback performs no format conversion.
