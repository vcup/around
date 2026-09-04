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
Implements the `Codec` Slot (`#[stabby::stabby] trait Codec`):
`probe(header, filename)` returns 0–100 confidence,
`name()` returns the extension‑match key,
`open(reader, out_stream)` produces an opaque stream handle.
Registered as a `DynCodecRef` (owning trait object) into a
`CodecRegister`. All methods dispatch through stabby ABI‑stable
vtables — single indirect jump per call.
_Avoid_: Decoder (decode is the action; codec is the component).

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

The complete PCM format used at a Stream/FilterChain Seam: sample rate,
channel count, `PcmEncoding`, `ByteOrder`, and interleave mode. `SampleSpec`
is the sole format value used by Codec output, Filter declarations,
OutputPlanner, and OutputBinding validation.

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
<!-- around-engine: pipeline, IPC, extension management, configuration;
     extension loading lives in around-extensions -->
<!-- (future: crates/around-engine/CONTEXT.md, extends into crates/around-extensions/CONTEXT.md) -->
<!-- ================================================================= -->

The `around-extensions` crate provides the generic extension framework:
`#[slot]` proc‑macro, Pull‑model export ABI (`AROUND_META` /
`AROUND_SLOTS` / `{slot_name}_create` registration functions),
`.so` lifecycle (`load` /
`load_bytes` / `unload` / `prepare_reload` / `commit_reload`),
progressive trust, dependency ordering, and signature verification.
Zero knowledge of specific extension types.
Reusable as a self‑contained generic framework;
depends on `stabby`, not on any `around‑*` crate.
## Extension

A dynamically‑loaded shared library that exports its capabilities through
static symbols (`AROUND_META`, `AROUND_SLOTS`), registration functions
(`{slot_name}_create`), and optional lifecycle functions (`around_init` /
`around_deinit`). Discovered via search‑path scanning or loaded on demand
through IPC. All extension traits use `stabby` for ABI‑stable dispatch.
Loaded and unloaded by `around-extensions`; slots and engines access the
resulting `Register`s through their concrete SDK types.
_Avoid_: Plugin, module.

## Slot

A trait defined in a domain‑specific SDK crate, annotated with `#[slot]`
(which wraps `#[stabby::stabby]`). The macro generates a `NAME`
constant, a `RegisterVTable` (`#[repr(C)]` fn‑pointer struct), a
`Register` storage struct, and an `impl RegisterVTable` for it. A Slot
is the contract that extensions and built‑in implementations commit to
when they register into its `Register`.
_Avoid_: Interface, API, hook point.

## Register

The concrete storage struct generated by `#[slot]` for one Slot.
Holds the collection of registered implementations as raw vtable+
instance pairs, uses `RegisterVTable` for push/remove, and exposes
Slot‑specific queries (`probe`, `for_each_entry`, …). One `Register`
per Slot — SDK init creates and
attaches it to `around-extensions`.

## SDK

A crate providing `#[slot]`‑annotated traits, their generated `Register`
types, and glue for one domain of slots. Extensions depend on the
relevant SDK, not on `around-engine`. SDKs are organised by domain:
`around-audio-sdk`, `around-gui-sdk`,
`around-net-sdk`, `around-analysis-sdk`, `around-storage-sdk`.
_Avoid_: API crate, plugin‑api.

## ExtensionMeta

A `#[stabby::stabby]` struct exported by every `.so` via the
`AROUND_META` static.  Contains identity fields (`name`, `semver`,
`api_version`), dependency declarations (`depends_on`,
`depends_on_slots`), author, and description — all string fields use
`&'static str`, fully const‑constructible from string literals.
The struct's pointer is the extension's identity throughout the
framework.

## AROUND_META / AROUND_SLOTS

Two `#[no_mangle] pub static` symbols that every `.so` extension
may export (only `AROUND_META` is required).  `AROUND_META` provides
`ExtensionMeta` (identity + dependencies).  `AROUND_SLOTS` defines
new Slots via a `Slice<'static, SlotDef>` (carrying a `RegisterVTable`
and slot name).  `SlotDef.slot_name` is `&'static str` — fully
const‑constructible from a string literal.

`AROUND_ENTRIES` (the original third symbol for registering
implementations) is **not used** — stabby trait‑object construction is
not const‑fn, so vtable assembly cannot happen at compile time for a
`static` initializer.  It is replaced by `{slot_name}_create`
registration functions.

## `{slot_name}_create` registration function

A `#[no_mangle] pub unsafe extern "C" fn` exported by an extension that
provides Slot implementations.  Takes an index (`usize`) and returns an
opaque pointer (`*mut c_void`); the host calls `create(0)`, `create(1)`,
… until null.  Each returned pointer is passed through
`RegisterVTable::push_raw` into the corresponding Register.  Replaces
`AROUND_ENTRIES`, avoiding const‑init constraints on stabby vtables.
Names follow the pattern `around_{slot_name}_create` (lowercase, e.g.
`around_codec_create`).


## Extension index

A `HashMap<Box<str>, (PathBuf, ExtensionMeta)>` built by
`Framework::scan()` at startup.  Contains metadata for every discovered
extension without performing a full load — only `dlopen` +
`dlsym("AROUND_META")` + `dlclose`.  Enables fast dependency lookup and
lazy loading decisions without loading every extension.

## Lazy loading

The strategy where `scan()` builds the index at startup but full loads
(`dlopen` + `around_init` + `{slot}_create`) are deferred until an
extension's functionality is explicitly requested or needed by a
dependency chain.  Keeps start‑up cost proportional to the number
of files on disk, not the number of loaded extensions.
## Framework

The central singleton in `around-extensions` that manages all loaded
extensions.  Contains the `slot_map` (opaque `RegisterVTable` references),
`loaded` extension list, lifecycle Registers, and dependency graph.
The framework knows **no concrete Slot types** — entries are stored as
opaque `(*const (), *mut ())` pairs inside each SDK's Register.
Accessible from any extension's `around_init` via
`around_extensions::framework()`.

## RegisterVTable

A `#[repr(C)]` struct of two `unsafe extern "C" fn` pointers —
`push_raw` and `remove_by_meta` — generated by `#[slot]` for each
Slot trait.  The `Register` struct holds a `&'static RegisterVTable`
and `around-extensions` calls through it to manage entries.

## Engine

The long-lived background process that manages the playback pipeline,
responds to IPC commands, and coordinates codec and source loading. The
`Engine` struct in `around-engine` is its concrete representation. An
Engine runs independently of any UI — UIs connect to and disconnect from
it without interrupting playback.
_Avoid_: Player, daemon, server, around process.

## OutputDevice

An output target known to the Engine, with a stable identity, category,
priority, and supported native `SampleSpec` modes. An OutputDevice describes
capabilities, not an active CPAL stream; the active Stream attachment is an
OutputBinding.

_Avoid_: AudioSink, CpalSink, output config — these describe an Adapter or
configuration, not the OutputDevice concept.

## OutputBinding

The active attachment between one Stream and one OutputDevice mode. An OutputBinding owns the output stream, callback lifetime, and transport buffer for one binding. It accepts only PCM blocks whose `SampleSpec` exactly matches the bound native mode; it never resamples, changes channels, changes interleave, or changes encoding.

_Avoid_: AudioSink — OutputBinding is the device attachment and its lifecycle.

## OutputPreference

An ordered Stream preference for selecting an OutputDevice and native mode. A preference may select an exact device, name, category, or any device, and may request a mode or recovery policy. Preference order and device priority are evaluated before conversion loss; the original list is retained across fallback migration.

## OutputPlanner

A pure Engine Module that validates a DeviceSnapshot and evaluates OutputPreference values against supported native modes. It returns one immutable OutputPlan containing the exact target `SampleSpec`, preference rank, device priority, conversion loss, and recovery policy. It has no CPAL handles, clocks, locks, or callbacks.

## OutputSession

The Engine Module that coordinates OutputDevice discovery, OutputPlanner, FilterChain construction, OutputBinding ownership, backpressure, and device-loss migration for one Stream. It preserves ordered OutputPreference values and rebuilds the FilterChain and OutputBinding when the selected native mode changes.

_Avoid_: output manager — OutputSession is the Stream’s output lifecycle, not a global device registry.

## Stream

A single playback context within the Engine: one Source, one decode loop,
one FilterChain, and one OutputSession owning an OutputBinding. The Engine
manages 0..N concurrent Streams. Each Stream has a unique `StreamId`, an
optional user-assigned alias, and an `output_spec` resolved when the
OutputSession binds a native OutputDevice mode. The decode loop may run on
the synchronous thread or async runtime; codec I/O and terminal conversion
retain their explicit scheduling contracts.

_Avoid_: Track, playback session, player instance.

## StreamId

A monotonically incrementing `u64` assigned by the Engine when a Stream
is created. Used by IPC clients to target commands (`Pause`, `Seek`,
`Stop`) to a specific Stream. When exactly one Stream is active, the
`stream_id` field may be omitted — the Engine implicitly targets the
sole active Stream.
_Avoid_: Track ID, handle.

## FilterChain

An ordered list of user Filters applied inside one Stream decode loop. Format
negotiation propagates through the chain and selects its terminal output. When
that output differs from `output_spec`, FilterChain adds the required terminal
conversion; otherwise it forwards the final Filter output directly. With no
user Filters, the Codec output format is the candidate. FilterChain owns sample
rate conversion, channel mapping, interleave conversion, byte order, and PCM
encoding whenever those transformations are needed. It reuses the Stream's
PcmBuffer; it does not hand conversion to an OutputBinding.

_Avoid_: Effect chain, audio processing pipeline.

## SampleSpec

Describes a complete uncompressed PCM format: sample rate (Hz), channel
count, `PcmEncoding`, `ByteOrder`, and interleave mode (interleaved or
planar). Used by Codec output, Filter `formats_in`/`formats_out`
declarations, and `output_spec` on a Stream. The format negotiation Module
uses `SampleSpec` values to find globally optimal assignments across the
FilterChain and to validate OutputBinding writes.

_Avoid_: Audio format, output config.

## AudioBuffer

The in-process byte view passed from FilterChain to OutputBinding. It carries
an exact `SampleSpec`, frame count, and frame-aligned bytes. The format and
storage must agree; OutputBinding rejects a mismatched AudioBuffer.

## PcmBuffer

A pre-allocated workspace used inside the decode loop. It contains a decode
region, two f32 processing regions, and an encoded byte region sized from the
selected output_spec. The workspace is reused every iteration and is rebuilt
only when an OutputSession changes the native mode.

## PlanarBuffer

A multi-channel audio buffer where each channel occupies a contiguous region
within one shared allocation. It is a Rust-side helper for planar Filter
processing; terminal conversion currently emits planar bytes but Codec output
is normalized to interleaved f32.

## TerminalConversion

The format-conversion stage owned by FilterChain after user Filters. It
converts the negotiated terminal format into the selected OutputDevice
`SampleSpec` when they differ, including sample rate, channel count,
interleave, byte order, and PCM encoding. It uses stateful linear
interpolation and pre-allocated PcmBuffer regions; it is not an OutputBinding
responsibility. Matching final Filter output follows the direct path.

_Avoid_: device resampler, sink conversion.

## Filter

A single audio-processing unit (EQ, compressor, volume, or another DSP
operation). The format-aware Filter Slot declares `formats_in` and
`formats_out`, receives an `AudioBufferC` with explicit input/output
`AudioFormatC`, and returns produced frames. The Engine owns both byte buffers
for the call; a Filter may not retain them. Registered dynamic Filters remain
an explicit follow-up integration, while built-in Filters are held directly by
FilterChain.

_Avoid_: Plugin, DSP unit, effect.

## Router

The future Engine subsystem that maps each Stream's ring buffer output
to one or more consumers (physical audio devices, virtual loopback
devices, network sinks, file recorders). The Router enables "any Source
to any combination of output sinks". In the current 004 branch, each
Stream has a single consumer (the CPAL output callback) and the Router
is implicit.
_Avoid_: Mixer, dispatcher, output manager.


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
