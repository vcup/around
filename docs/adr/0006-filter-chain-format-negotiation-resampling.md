# ADR 0006: FilterChain — Format-Negotiated Audio Processing with Pluggable Filters

The FilterChain ends at the bound OutputBinding's `SampleSpec`. It negotiates
the final Filter format and either forwards matching output or converts it.
Runtime-loadable Filters remain a separate integration seam.

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

Playback obtains the native `SampleSpec` when it binds an OutputDevice.
OutputSession stores the resulting OutputPlan and builds the FilterChain to
that spec. On device loss or capability changes, it replans and rebuilds the
FilterChain and OutputBinding, then resumes at the next decode block. Queued
PCM is discarded rather than replayed into a different format.

### 2. FilterChain — ordered filter list with format negotiation

```
Decode[file_fmt] → [user Filters] → terminal step → ring[output_spec] → CPAL
                                      │
                                      ├─ matching output: forward
                                      └─ otherwise: convert
```

The last Filter's declared output formats are the terminal candidates; the
candidate must be reachable through the preceding Filters. With no user
Filters, the Codec format is the candidate. The terminal step forwards a
candidate equal to `output_spec`; otherwise it performs the required rate,
channel, interleave, byte-order, or encoding conversion. FilterChain owns
these conversions. OutputBinding only validates and queues bytes matching its
native `SampleSpec`.

The chain retains the user Filter list. Conversion loss is scored over the
negotiated terminal format, after preference rank and device priority.

#### 2.1 Format scoring

OutputPlanner evaluates every supported native mode for the ordered
`OutputPreference` list. It ranks preference first, device priority second,
conversion loss third, then quality rank, default-mode status, sample rate,
device ID, and mode ID.

The planner selects a device mode, not an independent virtual format. The
chain negotiates its terminal format against that mode. With the current empty
user Filter list, the Codec format is the terminal format and the existing
source-to-target loss calculation applies.

#### 2.2 Examples

```
File: 8000 Hz mono f32 → OutputDevice: 44100 Hz stereo i16
Chain: decode f32 → terminal conversion (rate + channels + encoding)

File: 48000 Hz stereo f32 → OutputDevice: 48000 Hz stereo f32
Chain: decode f32 → terminal step forwards
```

### 3. PcmBuffer — pre-allocated workspace

The decode loop owns one `PcmBuffer` per Stream: a decoded f32 region, two
processing regions, and an output region sized for `output_spec`. FilterChain
reuses it for every block and never grows it during playback.

The terminal step writes frame-aligned bytes, including three- and six-byte
integer encodings. No per-block collection allocation is permitted;
stateful resampling history is allocated when the chain is built.

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

### 6. Volume

`Volume` is a zero-copy, interleaved-f32 Filter in the post-fader position.
Terminal processing remains outside user Filter state.

### 7. Filter trait — `#[stabby::stabby]`

The ABI carries explicit input/output formats and host-owned byte buffers.
Filters may not retain the buffers.
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

The format-aware ABI is ready for extension loading. Dynamic Filter discovery,
configuration, and instantiation into a Stream remain separate integration work.

### 8. ExtensionManager

Registration follows ADR-0004 §5. A single `.so` may export several
`{slot}_create` functions, so one extension can provide a Codec and Filters.
SDK Registers own the trait objects; `remove_by_meta` drops them before
`dlclose`.


### 9. Runtime Filters and configuration

FFmpeg and other runtime Filter bundles, plus Filter configuration and
instantiation, are future work. The current OutputSession supplies no user
Filters; the terminal step converts only when the Codec format differs from
the selected native mode.

### 10. Device-loss recovery

On `Lost`, auto-reconnect replans the native mode, creates a new binding,
resets terminal state, and resumes at the next decode block. The queued block
is discarded. Failed replanning enters `Error`. The current Engine exposes one
ordered `OutputPreference` per Stream.

## Consequences

### Code changes

- `pipeline.rs`: decode loops submit f32 blocks to OutputSession.
- `filter_chain.rs`: terminal negotiation and exact output bytes.
- `output_plan.rs`: native-mode planning and conversion-loss scoring.
- `output_session.rs`: planning, binding, backpressure, and recovery.
- `cpal_adapter.rs`: native capability discovery and exact-format binding;
  no conversion.
- `types.rs`: complete `SampleSpec`.
- `filter.rs`: format-aware Filter ABI.
### What this ADR does NOT decide

- The Router (multi-consumer fan-out, beyond this branch).
- The Source trait's async design (decided in ADR-0005, awaiting implementation).

### ADR relationships

- **ADR-0004** (Unified Extension System): Filter uses `#[stabby::stabby]`
  trait plus `{slot_name}_create` registration.
- **ADR-0005** (single-Engine multi-Stream): this ADR refines StreamState,
  FilterChain design, and decode-loop scheduling.
- Future ADR: async Source trait (io_uring / network streaming).
