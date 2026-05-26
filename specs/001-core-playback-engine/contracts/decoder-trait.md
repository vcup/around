# Contract: Decoder Trait

**Version**: 0.1.0-pre
**Stability**: Pre-1.0 (may evolve; see constitution API Stability Path)

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

    /// Read the next chunk of decoded PCM samples as interleaved f32 in [-1.0, 1.0].
    /// Decoder is responsible for converting native sample format to f32.
    /// Output sink converts f32 → device format at the pipeline boundary.
    /// Returns Ok(None) when the stream is exhausted.
    fn read(&mut self, buf: &mut [f32]) -> Result<Option<usize>, AroundError>;

    /// Seek to a byte offset within the stream (if supported).
    fn seek(&mut self, offset: u64) -> Result<(), AroundError>;

    /// Extract metadata from the source.
    fn metadata(&self) -> &Metadata;

    /// Return the output format (sample rate, channels).
    fn output_format(&self) -> SampleSpec;
}
```

## Contract

Every implementation of the `Decoder` trait MUST satisfy:

1. **Format Honesty**: `supported_formats()` MUST list every format this decoder can handle. A decoder claiming to support MP3 MUST successfully decode any compliant MP3 bitstream.

2. **Source Requirements**: `source_requirements()` MUST accurately declare all source capabilities the decoder depends on. If the source does not satisfy these requirements, the engine MUST reject the decoder-source pairing before `open()` is called, returning `AroundError::SourceIncompatible`. No requirement bits set means the decoder works with any source.

3. **Idempotent Detection**: `can_decode()` MUST return the same result for the same source regardless of call order or engine state. It MUST NOT consume bytes from the source.

4. **Open-or-Fail**: `open()` MUST either return a valid decoder ready to read or an `AroundError`. Opening the same source twice (two `open()` calls) MAY fail or produce independent decoder instances; either behavior is valid.

5. **PCM Output — f32 Interleaved**: The pipeline internally uses f32 as its universal sample format to eliminate format negotiation and conversion overhead between pipeline stages. `read()` MUST produce interleaved f32 samples in the range `[-1.0, 1.0]`. The decoder is responsible for any format conversion (e.g., i16 → f32). The output sink (cpal callback) is responsible for the final conversion from f32 to the device's native format. Format conversion only occurs at pipeline boundaries, never internally.

6. **Seek Contract**: If `seek()` returns `Ok(())`, the next `read()` call MUST produce samples from the target offset. If `seek()` returns `Err`, the decoder state is undefined and SHOULD be discarded.

7. **Metadata Availability**: `metadata()` MUST be callable immediately after `open()` returns `Ok` and at any point during decoding. It MUST NOT block or perform I/O beyond what was done during `open()`.

8. **Thread Safety**: `Send + Sync` is required. Multiple threads MAY call `metadata()` and `output_format()` concurrently; `read()` and `seek()` MUST be called from a single thread.

9. **No Panic**: `read()` on a corrupt stream MUST return `Err(AroundError::DecodeError {..})`, NOT panic. Panics in extension code MUST be caught at the trait boundary (constitution error handling constraint).

## Test Fixtures

Contract tests require:
- A minimal valid file for each format (e.g., `example.wav`)
- A truncated/corrupt file (e.g., `truncated.wav`)
- A file of a different format to test negative detection
- A dummy Source that advertises and does NOT advertise specific capabilities, to verify `source_requirements()` gating
