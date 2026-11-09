# Contract: Decoder Trait

**Version**: 0.1.0-pre
**Stability**: Pre-1.0 (may evolve; see constitution API Stability Path)

### Backward Compatibility

Pre-1.0 (v0.x): No compatibility guarantees between versions. Extensions must be recompiled for each minor version. v1.0+: Semantic versioning — major version bumps indicate breaking changes requiring recompilation; minor version bumps are backward compatible.

## Trait Definition

```rust
use bitflags::bitflags;

bitflags! {
    pub struct SourceRequirements: u8 {
        const NONE               = 0;
        const SEEKABLE           = 1 << 0;
        const KNOWN_LENGTH       = 1 << 1;
        const KNOWN_CONTENT_TYPE = 1 << 2;
    }
}

pub trait Decoder: Send + Sync {
    /// Returns the MIME types and format signatures this decoder handles.
    fn supported_formats() -> &'static [FormatSignature];

    /// Declares what capabilities the Source must provide for this decoder to function.
    /// The engine MUST verify the source satisfies these requirements before calling open().
    /// Example: FLAC decoder requires SEEKABLE | KNOWN_LENGTH.
    fn source_requirements() -> SourceRequirements;

    /// Quick check: can this decoder handle the given source? (magic bytes / extension)
    fn can_decode(source: &dyn Source) -> bool;

    /// Open the source and create a decoding stream.
    fn open(source: Box<dyn Source>) -> Result<Self, AroundError>
    where Self: Sized;

    /// Read the next chunk of decoded PCM samples as interleaved f32 in
    /// [-1.0, 1.0]. Returns `Ok(None)` when the stream is exhausted.
    /// The decoder converts from its native sample format; FilterChain selects
    /// the terminal output format and forwards or converts it to the bound
    /// OutputBinding (ADR-0006).
    fn read(&mut self, buf: &mut [f32]) -> Result<Option<usize>, AroundError>;

    /// Seek to a sample frame offset within the decoded PCM stream.
    /// The engine converts milliseconds to sample frames before calling this method.
    fn seek(&mut self, offset: u64) -> Result<(), AroundError>;

    /// Extract metadata from the source.
    fn metadata(&self) -> &Metadata;

    /// Return the output format (sample rate, channels).
    fn output_format(&self) -> SampleSpec;
}
```

> **Note — Trait Split**: This contract describes the full API surface. In practice, methods are split across two traits:
>
> - `Decoder` (object-safe, 4 methods): `read`, `seek`, `metadata`, `output_format`
> - `DecoderFactory` (non-object-safe, 4 methods): `supported_formats`, `source_requirements`, `can_decode`, `open`
>
> Total: 8 methods (4 + 4).

## Contract

Every implementation of the `Decoder` and `DecoderFactory` traits MUST satisfy:

1. **Format Honesty**: `supported_formats()` MUST list every format this decoder
   handles. A decoder claiming MP3 support MUST successfully decode any compliant
   MP3 bitstream.

2. **Source Requirements**: `source_requirements()` MUST declare every source
   capability the decoder needs. The engine MUST reject an incompatible
   source before `open()`, returning `AroundError::SourceIncompatible`. With no
   requirement bits, any source is valid.

3. **Idempotent Detection**: `can_decode()` MUST return the same result for the
   same source regardless of call order or engine state. It MUST NOT consume
   source bytes.

4. **Open-or-Fail**: `open()` MUST return either a decoder ready to read or an
   `AroundError`. Opening the same source twice MAY fail or produce independent
   instances.

5. **PCM Output — f32 Interleaved**: The pipeline uses interleaved f32 samples in
   `[-1.0, 1.0]`. `read()` MUST produce that format; the decoder converts from
   native sample formats. FilterChain selects the terminal format and forwards or
   converts samples to the bound OutputBinding. Conversion occurs at these
   boundaries, not between pipeline stages.

6. **Seek Contract**: If `seek()` returns `Ok(())`, the next `read()` MUST start
   at the target frame. After `Err`, decoder state is undefined and SHOULD be
   discarded.

7. **Metadata Availability**: `metadata()` MUST be callable immediately after
   successful `open()` and throughout decoding. It MUST NOT block or perform I/O
   beyond `open()`.

8. **Thread Safety**: `Send + Sync` is required. `metadata()` and
   `output_format()` MAY run concurrently; `read()` and `seek()` MUST run on one
   thread.

9. **No Panic**: `read()` on a corrupt stream MUST return
   `Err(AroundError::DecodeError {..})`, not panic. Panics in extension code MUST
   be caught at the trait boundary.

## Test Fixtures

Contract tests require:
- A minimal valid file for each format (e.g., `example.wav`)
- A truncated/corrupt file (e.g., `truncated.wav`)
- A file of a different format to test negative detection
- A dummy Source that advertises and does NOT advertise specific capabilities, to verify `source_requirements()` gating
