# ADR 0002: Fn-Pointer VTable over `dyn Codec`

The codec hot path uses a hand-rolled fn-pointer vtable (`AudioStreamVTable`)
rather than a `dyn Codec` trait object. This eliminates virtual dispatch
overhead from the per-frame `read()` call and satisfies Constitution §III
(Zero-Overhead Extensibility).

## Status

Accepted

## Context

The engine's decode loop calls `stream.read(&mut buf)` for every audio
buffer (typically every ~10ms at 44100 Hz). With a `dyn Codec` trait object,
this would be:

```rust
// Rejected approach:
trait Codec {
    fn read(&mut self, buf: &mut [f32]) -> Result<Option<usize>, AroundError>;
}
// Each call: vtable lookup → indirect jump.
```

In Rust, `dyn Trait` method calls go through two indirections: the fat pointer's
vtable pointer, then the vtable entry. For a call that fires thousands of times
per second, this is measurable overhead. Constitution §III mandates "zero-overhead
abstraction" and explicitly lists fn-pointer vtables as an approved pattern.

Three alternatives were considered:

### Alternative 1: `dyn Codec` trait object (rejected)

Simplest to implement and understand. Every Rust developer knows `dyn Trait`.
However:
- Double indirection on every `read()` call — the single hottest path in the
  application.
- `dyn Codec` requires object safety, which constrains the trait's evolution
  (no associated types, no generics in method signatures).

### Alternative 2: `enum_dispatch` (rejected)

`enum_dispatch` generates a concrete enum over all known codec types and
dispatches via a match. Zero overhead, fully static. However:
- All codec types must be known at compile time. around supports runtime-loaded
  codec extensions (`LoadCodec`, `LoadCodecBytes`), making the set of codecs
  open and unbounded. `enum_dispatch` cannot represent an open set.

### Alternative 3: Generic + monomorphized Engine<C: Codec> (rejected)

Makes the Engine generic over a concrete codec type. Zero overhead, fully static.
However:
- The engine holds a `CodecRegistry` containing multiple codecs, selected at
  runtime based on format detection. A single monomorphized type parameter
  cannot represent "one of N codecs chosen at runtime."

## Decision

Use a hand-rolled fn-pointer vtable:

```rust
pub struct AudioStreamVTable {
    pub read: unsafe fn(*mut c_void, buf: &mut [f32]) -> Result<Option<usize>, AroundError>,
    pub seek: unsafe fn(*mut c_void, frame: u64) -> Result<(), AroundError>,
    pub drop: unsafe fn(*mut c_void),
}

pub struct AudioStream {
    data: *mut c_void,                  // opaque codec state
    vtable: &'static AudioStreamVTable, // immortal, immutable
    pub sample_rate: u32,               // populated by codec during open()
    pub channels: u8,
    pub total_frames: u64,
}
```

Each codec defines a `static VTABLE: AudioStreamVTable` with its concrete
functions cast to the fn-pointer signatures. The engine calls:

```rust
fn read(&mut self, buf: &mut [f32]) -> Result<Option<usize>, AroundError> {
    unsafe { (self.vtable.read)(self.data, buf) }
}
```

This is a **single indirect call** — load the fn-pointer from the vtable (which is
a `&'static` reference, always in cache), call through it. No second vtable
lookup, no fat pointer.

### Unsafe boundaries

The vtable functions take `*mut c_void` and cast internally to the concrete
type. This is `unsafe` but encapsulated:

- `AudioStream::new()` is `unsafe` — the caller (codec's `open_fn`) asserts
  the data pointer matches the vtable.
- `AudioStream::read()` and `AudioStream::seek()` are `unsafe` internally but
  expose a safe public API — the invariant is upheld at construction time.
- `AudioStream::drop()` calls `vtable.drop` and nulls the data pointer to
  prevent double-free.

All `unsafe` is contained within `AudioStream` and each codec's vtable wrappers.
The engine and all higher layers only see safe `read()` / `seek()` calls.

### Codec registration

Codec crates export a `pub static INFO: CodecInfo` whose `open_fn` constructs an
`AudioStream`:

```rust
pub static WAV_INFO: CodecInfo = CodecInfo::new("wav", WAV_FORMATS, wav_open);
```

The engine calls `registry.push(WAV_INFO)` explicitly — no linker magic, no
build-script scanning. The `CodecInfo` carries an `open_fn:
fn(Box<dyn ReadSeek + Send>) -> Result<AudioStream, AroundError>` — a concrete
function pointer, not a trait object.

## Consequences

### Positive

- Single indirect call on the hot path (vs. double for `dyn Trait`).
- No trait object safety constraints — vtable functions take `*mut c_void`,
  so codec state types can be anything.
- `AudioStream` is `Send + Sync` (manually implemented via unsafe impls).
- Codec registration is explicit and auditable — grep for `registry.push`.
- The pattern compiles to the same (or fewer) instructions as directly-inlined
  code, per Constitution §III.

### Negative

- Every new codec must write `unsafe` vtable wrapper functions and cast
  `*mut c_void` correctly. A mistake produces UB, not a compile error.
- The pattern is non-idiomatic Rust — most developers expect `dyn Trait`.
  New contributors need the ADR to understand why.
- `AudioStream::new()` is `unsafe`, placing a correctness burden on codec
  authors.
- A `#[register_codec]` proc macro to generate vtable wrappers is planned
  but not yet implemented (see `around-codec-macros`).

## Related

- Constitution §III (Zero-Overhead Extensibility)
- ADR-0003 (IpcCodec trait + IpcWire enum over `Box<dyn IpcCodec>`)
- `crates/around-core/src/codec.rs` — `AudioStream`, `AudioStreamVTable`, `CodecInfo`
- `crates/around-codec-wav/src/lib.rs` — reference vtable implementation
