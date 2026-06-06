# ADR 0006: FilterChain — Format-Negotiated Audio Processing with Pluggable Filters

The decode pipeline is a FilterChain: an ordered sequence of audio-processing
filters terminated by a lock-free ring buffer (`ringbuf::HeapRb<f32>`). Every
filter—including resampling and de/interleaving—has equal status within the
chain. Format negotiation finds a globally optimal format assignment across
all filters and auto-inserts Resample, Deinterleave, and Interleave filters
at boundaries where formats diverge. Filters are runtime-loadable shared
libraries registered through the same extension framework that handles Codecs
(ADR-0004, `{slot_name}_create` registration functions). FFmpeg's `avfilter`
is available as an optional runtime plugin with zero-copy processing via a
shared buffer pool.

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

When `play()` is called the Engine queries the current output device for
its native `SampleSpec` (rate, channels, interleave mode). This is
stored as `StreamState.output_spec`. The FilterChain is constructed so
its final output format equals `output_spec`. On device reconnect
`output_spec` is re‑queried and the terminal Resample filter receives
the new ratio (rubato `set_resample_ratio` is lock‑free — no rebuild
needed).

### 2. FilterChain — ordered filter list with format negotiation

```
Decode[file_fmt] → [F₁] → [F₂] → … → [Fₙ] → ring[output_spec] → CPAL
                     ↑       ↑           ↑
               format negotiation at each boundary
```

**Resample, Deinterleave, and Interleave are filters — not a separate
pipeline stage.** Each filter declares its acceptable input/output
formats via a `formats_in` / `formats_out` list. The chain builder
constructs the final filter list at construction time:

1. Start with the user-supplied filter list.
2. Walk forward: for each adjacent pair `(Fᵢ, Fᵢ₊₁)`, find the
   intersection of `Fᵢ.formats_out` and `Fᵢ₊₁.formats_in`.
   If empty → auto-insert a Resample filter.
3. After the last user filter, compare its output format to
   `output_spec`. If they differ → auto-insert a terminal Resample.
4. When the FilterChain is empty (plain playback) and
   `file_fmt ≠ output_spec`, a single Resample is the sole filter.
5. Deinterleave and Interleave filters are auto-inserted at any
   boundary where the interleave mode changes.

#### 2.1 Format scoring

When multiple format choices exist for a given filter pair (intersection
of lists), the chain builder selects the globally optimal assignment.
Each candidate format pair is scored against the decode input format
with a penalty function. The engine **minimises total penalty** across
all filters:

| change | penalty | rationale |
|---|---|---|
| downsampling (rate _drop_) | 100 × kHz diff | irreversible quality loss |
| downmixing (channel _drop_) | 80 × channel diff | information loss |
| upsampling (rate _rise_) | 10 × kHz diff | cheap interpolation |
| upmixing (channel _rise_) | 5 × channel diff | cheap duplication |
| planar ↔ interleaved | 1 | cheap reorder |

The global optimum is the format assignment with the lowest total score.
If two assignments are tied, the one with the highest sample rate wins.

#### 2.2 Examples

```
File: 8000 Hz mono → Output: 44100 Hz stereo
  No user filters.
  → auto-insert Resample(8000→44100, 1ch→2ch)
  Penalty: 10×36 + 5×1 = 365

File: 44100 Hz stereo → EQ(interleaved:in, interleaved:out) → Output: 48000 Hz stereo
  All filters accept interleaved.
  → auto-insert terminal Resample(44100→48000)
  Penalty: 10×3.9 = 39

File: 44100 Hz stereo → EQ(planar:in, planar:out) → Compressor(planar:in, interleaved:out) → Output: 48000 Hz stereo
  → auto-insert Deinterleave after decode
  → auto-insert Interleave after Compressor
  → auto-insert terminal Resample(44100→48000)
  Penalty: 1 (deinterleave) + 1 (interleave) + 39 (upsample) = 41
```

### 3. PcmBuffer — single allocation, region layout

The decode loop operates on one pre‑allocated `Vec<f32>`:

```
pcm_buf = Vec<f32> with capacity = decoder_max_samples + resample_max_samples

region 0: [0 … decoder_max_samples-1]       — decode output + in-place filter
region 1: [decoder_max_samples … capacity-1] — resample output (when needed)
```

| path | usage |
|---|---|
| Plain playback (fmt match) | decode → region 0 → ring.push_slice(region 0) |
| + Resample filter | decode → chain in‑place → resample reads region 0, writes region 1 → ring.push_slice(region 1) |
| + planar filters | auto‑inserted Deinterleave writes separate planar buffer; Interleave writes back to region 0 |

`decoder_max_samples` is determined by `AudioStream.max_frames_per_read`
(multiplied by channels). The resample region is sized by the chain
builder during negotiation (`decoder_max_samples × max_rate_ratio`).

**Zero allocations per iteration.** `pcm_buf` is allocated once.
`ringbuf::push_slice` memcpy is the sole data movement.

### 4. Ring buffer — `ringbuf::HeapRb<f32>`

`ringbuf::HeapRb<f32>` (lock‑free SPSC FIFO; 13 M downloads, v0.5.0)
replaces the current `crossbeam::bounded<Vec<f32>>`. Rationale:

| | `ringbuf` | `rt-ring` (rejected) |
|---|---|---|
| full push behaviour | `Err(Full)` — explicit | overwrite oldest — silent data loss |
| audio fit | ✅ backpressure detection | ❌ overwrite causes skip without signal |
| maturity | 13 M downloads, stable | 89 downloads, 1 release |

When `push_slice` returns `Err(Full)`, the decode loop enters a spin‑and‑retry
loop (or skips one iteration and logs an underrun). Under normal operation the
ring is sized so it never fills (4 batches of worst‑case audio data — 2 seconds
of jitter absorption).

**CPAL callback side:** `pop_slice(&mut data)` fills the CPAL output buffer
directly. Returns 0 when empty — callback fills the remainder with silence
(underrun).

### 5. Decode‑loop scheduling

```
tokio main runtime (IPC / UI tasks)
  │
  └─ decode_loop (async task, one per Stream)
       ├─ source.read(raw_buf).await   ← async I/O (io_uring, TCP)
       │                                  Yields to other tasks naturally
       └─ tokio::task::spawn_blocking {
            decoder.decode()            ← CPU (codec, possibly heavy)
            filter_chain.process(pcm)   ← CPU (DSP, possibly heavy)
            ring.push_slice(&samples)    ← memcpy, O(N)
          }.await
```

- **Async I/O** is on the main runtime, competing fairly with IPC handlers.
  No priority inversion.
- **CPU‑intensive work** is offloaded to `spawn_blocking` so it never starves
  IPC handlers.
- The blocking pool size is configurable; N concurrent streams consume at
  most N pool threads.
- CPAL output callback: system audio thread, completely independent of tokio.

### 6. Volume is a Filter

A `Volume` filter lives in the FilterChain (post‑fader position). It accepts
interleaved f32, multiplies every sample by a gain factor, zero‑copy.
Zero‑overhead when absent (not present in the filter list).

### 7. Filter trait — `#[stabby::stabby]`

stabby generates an ABI‑stable vtable with identical call overhead to
hand‑written C vtables — a single `call [rax+offset]` after one pointer
load. stabby also provides canaries and layout reports for cross‑compiler‑
version safety.

```rust
use stabby::dynptr;

#[stabby::stabby]
pub trait Filter {
    extern "C" fn process(&mut self, buf: &mut AudioBufferC) -> u32;
    extern "C" fn get_params(&self) -> stabby::string::StabbyString;
}
```

`dynptr!(Box<dyn Filter>)` at the call site compiles to the same
instructions as a hand‑written C vtable.

### 7.1 Filter registration

Filter registration uses the extension framework defined in ADR‑0004.
Each filter `.so` exports an `around_filter_create(index: usize) ->
*mut c_void` registration function (following the `{slot_name}_create`
pattern from ADR‑0004 §5.2).  The host polls `create(0)`, `create(1)`,
… until null; each opaque pointer is passed to the Filter Register via
`push_raw`.  The `Filter` trait itself carries the factory interface —
`info()` returns metadata (name, description, formats_in/out),
`open(config)` returns a `Box<dyn Filter>` instance, and `process()`
applies the filter.

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


### 9. FFmpeg bridge — runtime plugin, zero-copy

A separate crate `around-filter-ffmpeg` compiles to a shared library
(`.so`/`.dylib`/`.dll`). It links FFmpeg (`libavfilter`, `libavutil`)
**statically** — the host process has no compile‑time or link‑time
knowledge of FFmpeg. The Engine loads it through the unified
ExtensionManager path (section 8).

Inside the plugin:

- `avfilter_graph_alloc()` creates a lightweight container (a single
  `av_mallocz` call + default options). All filters within this `.so`
  share this one graph.
- `avfilter_graph_alloc_filter(graph, filter_def, name)` + `avfilter_init_str`
  (or `avfilter_init_dict` for KDL‑derived config) creates individual filter
  contexts. No `avfilter_graph_config` needed — standalone filters are
  driven directly via `process_frame`.
- **Custom allocator:** `av_set_mem_funcs(Rust allocator)` replaces all
  `av_malloc` / `av_realloc` / `av_free` with our heap functions.
- **Buffer pool:** `av_buffer_pool_init2(Rust pool, Rust alloc_callback, Rust free_callback)`
  ensures every `AVFrame.data[]` buffer is allocated within our allocation
  domain.
- **In‑place filters** (EQ, compressor, volume): `AVFrame.data[]` maps to
  our `PcmBuffer` slice → `process_frame` mutates in‑place → **zero copy**.
- **Re‑allocating filters** (resampler): the new `AVFrame` is pulled from
  the shared buffer pool. Its data pointer IS our memory — **zero copy at
  the library boundary** (one memcpy from old frame to new frame, as
  required by the resample operation itself).

Filter `drop` calls `avfilter_graph_free(&graph)`, which walks all filter
contexts, releases buffer pool references, and frees the graph.

### 10. Filter configuration — KDL + string dual format

FFmpeg‑style key=value strings are the native config language. KDL is
a structured front‑end for common filters, parsed to string by the Engine
at configuration time:

```kdl
// KDL structured — Engine parses to string
filter name="eq" {
    frequency "1000"; gain "3.0"; q "1.5"
}
// → "equalizer=f=1000:g=3.0:q=1.5"

// Raw string fallback — passed directly to open_fn
filter name="custom" args="acrossover=split=500 1500 4000:order=4th"
```

### 11. Device‑loss recovery

Per ADR-0005, when the output device changes:

1. `output_spec` is re‑queried from the new device.
2. The terminal Resample filter receives the new ratio via
   `set_resample_ratio` (rubato supports runtime ratio change, lock‑free,
   zero rebuild).
3. Per‑device‑type policy governs auto‑continue:

| device type pattern | behaviour |
|---|---|
| bluetooth headset | auto‑reconnect + auto‑continue |
| USB DAC | auto‑reconnect + pause (no auto‑continue) |
| onboard speaker | stop on loss (don't reconnect) |

Device type detection is unreliable at the driver level. The Engine
uses device‑name pattern matching with a user‑configurable policy file
as a pragmatic approximation.

## Consequences

### Code changes

- `pipeline.rs`: decode loop refactored to use `PcmBuffer` region layout
  and `ringbuf::HeapRb`. Static `to_vec()` allocation removed.
- `pipeline::filter_chain` (new module): format negotiation, filter graph
  builder, auto‑insert rules, scoring function.
- `extensions.rs`: added filter registration path via Pull‑model exports.
- `ipc/types.rs`: `SampleSpec` gains `interleave` field; `PlaybackState`
  (to become `StreamState` per ADR-0005) gains `output_spec` field.
- `output.rs`: `AudioOutput` replaced by `ringbuf::HeapRb<f32>`.
- `Cargo.toml`: removed `crossbeam` dependency (replaced by `ringbuf`);
  added optional `rubato` dependency; scaffolded `around-filter-ffmpeg`
  crate behind feature flag `ffmpeg-compat`.

### What this ADR does NOT decide

- The exact `PlaybackState → StreamState` migration (ADR-0005 scope).
- The Router (multi‑consumer fan‑out, beyond the 004 branch).
- The Source trait's async design (decided in ADR-0005, awaiting
  implementation).

### ADR relationships

- **ADR-0004** (Unified Extension System): Filter uses `#[stabby::stabby]`
  trait + `{slot_name}_create` registration, the same mechanism as Codec.
- **ADR-0005** (single‑Engine multi‑Stream): this ADR refines StreamState,
  FilterChain design, and decode‑loop scheduling.
- Future ADR: async Source trait (io_uring / network streaming).
