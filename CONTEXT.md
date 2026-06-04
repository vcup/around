<!--
  CONTEXT.md — project-level glossary. Currently single-context; terms are
  organised into sections corresponding to future bounded contexts. When a
  section accumulates enough terms (generally >5), it will split into its own
  per-crate CONTEXT.md and CONTEXT-MAP.md will be introduced.

  Growth rule: add one term per grilling session, when formally named,
  defined, and agreed. Never batch-add terms without discussion.
-->

# around

around is a cross-paradigm audio platform — local-first and offline-
capable by default, extensible to cloud services and federated social
features. It plays any audio content from any source, routes it to any
sink, and exposes every function through multiple control surfaces so
that no single interface or usage pattern is mandatory.

At its centre is a headless engine: long-running, UI-independent,
reachable via CLI, native IPC, HTTP, WebSocket, gRPC, and MPRIS.
Interfaces — terminal (TUI), desktop (GUI), web dashboard, mobile
companion — connect as clients without interrupting playback. The
engine's routing matrix treats output sinks uniformly, enabling use
cases from local speaker output to virtual-audio-device patching
(Voicemeeter-style) to real-time network streaming and multi-instance
synchronised playback over LAN or WAN. Recording is a first-class sink
direction, not an afterthought.

The platform ships with an open-source, self-hostable backend for
library synchronisation, discovery augmentation, and federated social
features — no walled garden, no mandatory cloud account.

## Language

**Documentation language**:
English is the default authoring language for all documentation and agent
output. Agent thinking and code comments default to English. Agent
conversations with the user may be in Chinese.
_Avoid_: Writing documentation directly in Chinese without maintaining an
English source of truth.

---

<!-- ================================================================= -->
<!-- around-core: foundational traits, types, and audio primitives     -->
<!-- (future: crates/around-core/CONTEXT.md)                           -->
<!-- ================================================================= -->

## Codec

A component that converts encoded audio bytes into raw PCM f32 samples.
Every codec implements the `CodecInfo` + `AudioStreamVTable` pattern: a
`pub static INFO: CodecInfo` registered by the engine via
`registry.push(INFO)`. The vtable uses fn-pointers for static dispatch —
no `dyn Trait` on the hot path.

Codecs may be built-in (compiled into the engine), loaded from shared
libraries at runtime (see Extension), or loaded from in-memory bytes.

## Source

The abstract trait (`around_core::Source`) representing where audio bytes
come from. Defines `open()` (returns `Box<dyn Read + Send>`),
`open_seekable()` (returns `Box<dyn ReadSeek + Send + Sync>`), and
`identifier()` (a human-readable string — may be a file path, URL, or
other label).

Implementations include `FileSource` (local files), with future support
for HTTP streams, in-memory buffers, and virtual filesystems.
_Avoid_: File, path, input — these are specific kinds of Source, not the
abstraction itself.

## PlaybackStatus

The engine's playback lifecycle state: `Playing | Paused | Stopped |
Buffering`. The canonical form is an enum. Currently the codebase uses
`u8` constants (`PLAYING`, `PAUSED`, `STOPPED`, `BUFFERING`) in
`around-core` and a separate `TrackState` enum (with an additional
`Error` variant) in the IPC layer — these are known deviations on the
`004-platform-native-ipc` branch.
_Avoid_: TrackState, PlaybackState (the struct holding current playback
metadata; distinct from the status enum).

## SampleSpec

The combination of sample rate, channel layout, and bit depth that
describes a PCM audio stream. Represented as a struct with `sample_rate:
u32`, `channels: u8`, `bit_depth: u8`.
_Avoid_: Audio format, stream config — these are broader concepts.

## ContentType

A string-tagged identifier for the type of audio content (`"music"`,
`"podcast"`, `"audiobook"`, `"radio"`, `"live_stream"`, `"ambient"`).
User-extensible via any string. Content type is a tag, not an
architectural boundary — all content types share the same data model.
_Avoid_: Genre, category — genre is metadata, ContentType is a behavioral
hint.

---

<!-- ================================================================= -->
<!-- around-engine: pipeline, IPC, extension management, configuration -->
<!-- (future: crates/around-engine/CONTEXT.md)                         -->
<!-- ================================================================= -->

## Engine

The long-lived background process that manages the playback pipeline,
responds to IPC commands, and coordinates codec and source loading. The
`Engine` struct in `around-engine` is its concrete representation. An
Engine runs independently of any UI — UIs connect to and disconnect from
it without interrupting playback.
_Avoid_: Player, daemon, server, around process.

## Stream

A single playback context within the Engine: one Source, one decode loop,
one optional FilterChain, and one output ring buffer. The Engine manages
0..N concurrent Streams. Each Stream has a unique `StreamId` and an
optional user-assigned alias. A Stream is created by a `Play` command
and destroyed by `Stop`, exhaustion, or error. The decode loop runs
asynchronously on the Engine's tokio runtime, feeding samples into a
lock-free ring buffer consumed by the output layer.
_Avoid_: Track, playback session, player instance.

## StreamId

A monotonically incrementing `u64` assigned by the Engine when a Stream
is created. Used by IPC clients to target commands (`Pause`, `Seek`,
`Stop`) to a specific Stream. When exactly one Stream is active, the
`stream_id` field may be omitted — the Engine implicitly targets the
sole active Stream.
_Avoid_: Track ID, handle.

## FilterChain

An ordered sequence of DSP filters (EQ, compressor, limiter, etc.)
applied to decoded PCM samples inline within the decode loop — after
decode, before the ring buffer write — to maximise L1/L2 data-cache
locality. The chain is per-Stream and may be empty.
_Avoid_: Effect chain, audio processing pipeline.

## Router

The future Engine subsystem that maps each Stream's ring buffer output
to one or more consumers (physical audio devices, virtual loopback
devices, network sinks, file recorders). The Router enables "any Source
to any combination of output sinks". In the current 004 branch, each
Stream has a single consumer (the CPAL output callback) and the Router
is implicit.
_Avoid_: Mixer, dispatcher, output manager.

## Extension

A codec distributed as a shared library (`.so`/`.dylib`/`.dll`) and
loaded at runtime by the `ExtensionManager` via `libloading`. Extensions
export a single FFI entry point (`get_codec_info()`) returning a
`CodecInfoFFI` descriptor. Extensions may be loaded from filesystem paths
(`LoadCodec`) or from in-memory bytes (`LoadCodecBytes`). The
`ExtensionSource` tag identifies how an extension was discovered
(built-in, static, dynamic, runtime, bytes, config, discovered).
_Avoid_: Plugin, addon, dynamic library — these are implementation
details.

## IpcCommand

A tagged enum (`serde(tag = "command")`) representing a control
instruction sent to the Engine. Variants include `Play`, `Pause`,
`Resume`, `Seek`, `Stop`, `Status`, `ListCodecs`, `LoadCodec`,
`LoadCodecBytes`, `Cleanup`, and `Shutdown`. Commands are delivered over
platform-native IPC (Unix socket or named pipe), TCP, or UDP, and are
encoded via an IpcCodec.
_Avoid_: Message, request, RPC — IpcCommand is specifically the incoming
command, distinct from the IpcResponse.

## IpcCodec

A transport-agnostic trait for encoding and decoding IPC messages. Two
implementations exist: `JsonLineCodec` (newline-delimited JSON, always
available) and `ProtoCodec` (varint-delimited protobuf, feature-gated
behind `protobuf-ipc`). `IpcWire` is the runtime-selectable enum that
dispatches to the active codec. The trait is async-only — all transports
(including Windows named pipes) use tokio async I/O.
_Avoid_: Serializer, wire format — these undersell the framing
responsibility.
