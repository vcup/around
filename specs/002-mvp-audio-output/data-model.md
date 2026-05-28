# Data Model: MVP Audio Output Pipeline & Critical Fixes

**Feature**: 002-mvp-audio-output
**Date**: 2026-05-28

## Entity Overview

No new data entities beyond what 001-core-playback-engine defines. This feature adds lifecycle states and connections between existing entities.

```text
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│  Decoder     │────▶│  Ring Buffer  │────▶│  cpal Output │
│  (decode     │     │  (PCM f32)    │     │  Stream      │
│   loop)      │     │               │     │  (audio dev) │
└──────────────┘     └──────────────┘     └──────────────┘
       │                      ▲
       │                      │ (error callback)
       ▼                      │
┌──────────────┐     ┌──────────────┐
│ Engine       │     │ device_lost  │
│ (running     │◀────│ flag         │
│  flag)       │     └──────────────┘
└──────┬───────┘
       │ (shared)
       ▼
┌──────────────┐
│ IPC Server   │
│ (Unix socket)│
└──────────────┘
```

## IPC Server Lifecycle States

| State | Description | Socket Status |
|-------|-------------|--------------|
| Stopped | No playback; no IPC server | File absent |
| Starting | cmd_play invoked, binding socket | 0600, created before audio |
| Playing | Audio output + IPC active | Open for connections |
| Draining | Playback ended, cleaning up | Being deleted |
| Stopped (clean) | Exited normally | File absent |

## Signals Affecting Lifecycle

| Signal | Effect | Socket Cleanup |
|--------|--------|----------------|
| SIGINT (Ctrl+C) | Set running=false, drain loop exits, cleanup runs | Yes |
| SIGTERM | Same as SIGINT | Yes |
| SIGHUP | Same as SIGINT | Yes |
| IPC stop command | Same as SIGINT | Yes |
| EOF (file complete) | Decoder returns Ok(None), loop exits, cleanup runs | Yes |
| cpal StreamError | Set device_lost=true, auto-pause | No (device error, not exit) |

## Validation Rules

- Socket path `/tmp/around.sock` must be writable by the current user
- If socket file exists at startup: attempt connect → success means another instance running (error); failure means stale socket (delete and rebind)
- Socket permissions must be 0600 (owner-only read/write)
