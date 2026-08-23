# ADR 0006: FilterChain — Format-Negotiated Audio Processing with Pluggable Filters

The decode pipeline is a FilterChain: an ordered sequence of user Filters
terminated by a byte OutputBinding. FilterChain owns terminal conversion and
produces exact native `SampleSpec` bytes before the OutputBinding ring.
Format planning chooses an OutputDevice mode, then FilterChain adapts Codec
output to that mode. Runtime-loadable Filters remain a separate integration
seam; the built-in terminal conversion is always present.

## Status

Accepted

## Context

### Problem 1: decode format ≠ output format

A CPAL output stream is bound to the OS audio device's native format
(e.g., 44100 Hz stereo f32). Source files can be any format (8000 Hz mono
u8, 96000 Hz stereo s16, …). Feeding decoded PCM directly into a
device-native CPAL callback corrupts audio — the two formats disagree on
sample count, channel count, and interleave layout.

### Problem 2: DSP filtering location

ADR-0005 places the FilterChain "inline in the decode loop" without
specifying where format adaptation occurs. Every filter has a preferred
input/output format; format requirements propagate through the chain.

### Problem 3: plugin filters without compile-time coupling

The Engine must not depend on FFmpeg at compile time, yet must support
zero-copy FFmpeg filter-graph processing when available.

## Decisions

### 1. `output_spec` on every Stream

When playback binds an OutputDevice, the Engine obtains a native
`SampleSpec` containing rate, channels, interleave, encoding, and byte order.
OutputSession stores the selected OutputPlan and builds the FilterChain so its
final output equals that exact `output_spec`. If the binding is lost or the
device capabilities change, OutputSession obtains a fresh snapshot, rebuilds
the plan, FilterChain, and OutputBinding, then resumes at the next decode
block. It does not mutate a live binding's format or replay queued PCM.

### 2. FilterChain — ordered filter list with format negotiation

```
Decode[file_fmt] → [user Filters] → terminal conversion → ring[output_spec] → CPAL
                                      │
                                      └─ exact rate/channel/layout/encoding bytes
```

Resampling, channel conversion, interleave conversion, byte order, and PCM
encoding are owned by FilterChain. OutputBinding only validates and queues
bytes already matching its native `SampleSpec`; it never converts them.
Filter declarations use `formats_in` / `formats_out` through the format-aware
Filter ABI. Dynamic Filter registration/configuration remains a separate
integration task; the built-in terminal conversion is always available.

The chain builder retains the user Filter list and adds terminal conversion
when the final format differs from `output_spec`. Conversion loss is scored
using the table below, and OutputPlanner evaluates that loss only after
preference rank and device priority.

#### 2.1 Format scoring

OutputPlanner evaluates each supported native `OutputDevice` mode for the
ordered `OutputPreference` list. Preference index is the primary ordering,
then device priority, then `ConversionLoss` over rate, channels, interleave,
byte order, and encoding. Native mode quality and default-mode status break
tied scores deterministically.

The planner does not choose a virtual format independently of a device:
`OutputBinding` supplies the exact native `SampleSpec`, and FilterChain
terminal conversion adapts Codec output to it.

#### 2.2 Examples

```
File: 8000 Hz mono f32 → OutputDevice: 44100 Hz stereo i16
Chain: decode f32 → TerminalConversion(rate + channels + encoding)

File: 48000 Hz stereo f32 → OutputDevice: 48000 Hz stereo f32
Chain: decode f32 → no terminal conversion
```

### 3. PcmBuffer — pre-allocated workspace

The decode loop owns one `PcmBuffer` workspace per Stream. It contains a
decoded f32 region, two f32 processing regions, and a byte output region sized
from the selected `output_spec`. FilterChain reuses these regions for every
decode block; the workspace is never grown during playback.

The terminal conversion writes exact packed bytes, including three- and
six-byte integer encodings when such a target is supplied by an Adapter.
OutputBinding receives only a frame-aligned view of the byte region.

**No per-block collection allocation is permitted in the production path.**
Stateful resampling history is allocated when FilterChain is built.

### 4. Ring buffer — byte transport

The OutputBinding Adapter owns a lock-free SPSC `ringbuf::HeapRb<u8>`.
Its capacity is derived from the bound `SampleSpec.bytes_per_frame()`.
The CPAL callback copies bytes from the ring into `Data::bytes_mut()` and
fills an underrun with encoding-correct silence. The callback performs no
format conversion or allocation.

When `push_slice` reports a full ring, the synchronous decode owner waits
with backpressure rather than dropping PCM. Device loss changes the binding
state; the Stream discards the possibly queued block and asks OutputSession
to re-plan rather than replaying bytes into a new format.

### 5. Decode-loop scheduling

```
tokio runtime (IPC / UI tasks)
  │
  └─ decode_loop (one per Stream)
       ├─ Codec read runs on the blocking pool for the current sync ABI
       ├─ FilterChain produces exact output bytes
       └─ OutputBinding queues bytes; CPAL callback consumes them
```

The current async entry point keeps orchestration asynchronous and moves Codec
I/O to `spawn_blocking`. A future async-native Codec contract may replace this
Adapter while preserving FilterChain and OutputBinding ownership. Synchronous
backpressure remains an explicit OutputBinding contract.

### 6. Volume is a Filter

A `Volume` Filter lives in the FilterChain (post-fader position). It accepts
interleaved f32, multiplies every sample by a gain factor, and is zero-copy.
The terminal conversion remains separate from user Filter state.

### 7. Filter trait — `#[stabby::stabby]`

stabby generates an ABI-stable vtable. The Filter ABI carries explicit
input/output format descriptors and byte buffers; the host owns both buffers
for the duration of the call and a Filter may not retain either pointer.
```rust
#[repr(C)]
pub struct AudioBufferC {
    pub input_data: *const u8,
    pub input_len: usize,
    pub output_data: *mut u8,
    pub output_len: usize,
    pub frames: usize,
    pub input_format: AudioFormatC,
    pub output_format: AudioFormatC,
}

#[stabby::stabby]
pub trait Filter {
    extern "C" fn name(&self) -> *const u8;
    extern "C" fn formats_in(&self) -> AudioFormatListC;
    extern "C" fn formats_out(&self) -> AudioFormatListC;
    extern "C" fn process(&mut self, buf: &mut AudioBufferC) -> u32;
    extern "C" fn reset(&mut self);
}
```

### 7.1 Filter registration

The format-aware Filter ABI is ready for extension loading, but the Engine
does not yet instantiate dynamic Filter entries into a Stream's production
FilterChain. Dynamic Filter discovery, configuration, and lifecycle remain a
separate follow-up; this ADR fixes the ABI contract and terminal conversion
ownership.

### 8. ExtensionManager — unified plugin registry

Registration mechanics are defined in ADR‑0004 §5.  This section describes
the audio‑specific payloads and how the Engine stores them.
A single `.so` may export multiple `{slot}_create` functions — one for
each Slot it implements — enabling distributable bundles that ship a
codec alongside a suite of filters in one file.

```rust
// SDK registries store typed trait objects:
//   CodecRegister → Vec<DynCodecRef>
//   FilterRegister → Vec<dynptr!(Box<dyn Filter + Send + Sync>)>
// Lifecycle managed by around‑extensions via RegisterVTable.
```

Loading follows ADR‑0004 §5.3 (recursive `load()`), which handles
dependency resolution, Slot definition, entry registration, and
lifecycle initialisation.  Trait objects are owned by SDK Registries.
`remove_by_meta` drops them before `dlclose`.


### 9. Runtime Filter integrations

FFmpeg and other runtime Filter bundles are future Adapters. This ADR does
not commit the repository to a particular FFmpeg allocator, frame ownership
scheme, or zero-copy implementation. The current committed implementation
provides the format-aware Filter ABI and built-in terminal conversion only.

### 10. Filter configuration

Filter configuration and dynamic Filter instantiation are future work. The
current OutputSession passes an empty user Filter list and always builds the
terminal conversion needed for the selected native OutputDevice mode.
### 11. Device-loss recovery

When an OutputBinding reports `Lost`, the Stream marks the loss and applies
its `RecoveryPolicy`. With auto-reconnect enabled, OutputSession obtains a
fresh DeviceSnapshot, plans a new native mode, creates a new OutputBinding,
resets TerminalConversion state, and resumes at the next decode block. The
possibly queued block is discarded; PCM is never replayed into a different
format. If re-planning fails, the Stream enters `Error`.

The current Engine exposes a single ordered OutputPreference per Stream.
Device categories and richer fallback policy can be added without changing
the OutputBinding Seam.

## Consequences

### Code changes

- `pipeline.rs`: sync and async decode loops submit decoded f32 blocks to
  OutputSession.
- `filter_chain.rs`: FilterChain owns terminal rate/channel/interleave/byte
  order/encoding conversion using pre-allocated PcmBuffer regions.
- `output_plan.rs`: pure OutputDevice capability model, preference ordering,
  and conversion-loss scoring.
- `output_session.rs`: OutputSession plus OutputDiscovery, OutputBinder,
  OutputBinding, and deterministic MemoryOutputAdapter.
- `cpal_adapter.rs`: CPAL capability discovery and exact native binding;
  no conversion occurs in the Adapter.
- `types.rs`: SampleSpec explicitly contains PcmEncoding and ByteOrder.
- `filter.rs`: Filter ABI carries input/output AudioFormatC and byte buffers.
### What this ADR does NOT decide

- The Router (multi-consumer fan-out, beyond this branch).
- The Source trait's async design (decided in ADR-0005, awaiting implementation).

### ADR relationships

- **ADR-0004** (Unified Extension System): Filter uses `#[stabby::stabby]`
  trait plus `{slot_name}_create` registration.
- **ADR-0005** (single-Engine multi-Stream): this ADR refines StreamState,
  FilterChain design, and decode-loop scheduling.
- Future ADR: async Source trait (io_uring / network streaming).
