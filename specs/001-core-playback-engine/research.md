# Research: Core Audio Playback Engine

**Feature**: 001-core-playback-engine
**Date**: 2026-05-26
**Status**: Complete

## Decision: WAV Decoding Library

**Decision**: Use `symphonia` for WAV parsing.

**Rationale**: `symphonia` is a pure-Rust, zero-unsafe audio decoding framework supporting multiple container formats (WAV, OGG, FLAC, MKV, ISO/MP4). Choosing it over `hound` (WAV-only) gives two advantages: (1) it means adding FLAC and OGG Vorbis support later requires only adding `symphonia` feature flags rather than introducing a new library, and (2) its `FormatReader` trait is close to the `Decoder` trait we define in `around-core`, making the contract implementation natural.

**Alternatives considered**:
- `hound` — simpler but WAV-only. Rejected because it creates a dead-end integration that we'd replace immediately for the next format.
- Direct WAV parsing — spec-compliant but wastes effort on a solved problem, introduces parsing bugs, and distracts from the engine architecture.

## Decision: Cross-Platform Audio Output

**Decision**: Use `cpal` as the audio output backend.

**Rationale**: `cpal` is the de facto standard cross-platform audio library for Rust, actively maintained, and provides a `Stream` abstraction that maps cleanly to our pipeline's output stage. It supports CoreAudio (macOS), PulseAudio/ALSA (Linux), and WASAPI (Windows). Its callback-based model (`build_output_stream`) fits the real-time audio thread requirement in FR-006.

**Alternatives considered**:
- `rodio` — higher-level (handles mixing, spatial audio), but too opinionated for our zero-overhead requirements. We need raw PCM buffer access, not an opinionated mixer.
- Direct platform APIs — maximal control, but 3x the maintenance burden. Reserved for optimization after the cross-platform baseline is validated.

## Decision: Extension Loading Mechanism

**Decision**: `libloading` for runtime shared library loading, with a compile-time shim crate pattern for zero-overhead dispatch.

**Rationale**: The constitutional mandate (Core Principle III, FR-003a) requires runtime-loaded extensions to match static-link performance. The shim pattern works as follows:
1. An extension compiles to a `.so`/`.dylib`/`.dll` that exports a `fn create_decoder() -> Box<dyn Decoder>` symbol.
2. At load time, `libloading` resolves this symbol once and stores a function pointer.
3. The shim wraps the function pointer in a concrete `struct ExtensionDecoder` that implements `Decoder` via direct FFI calls — no `dyn Trait` indirection per call.
4. For compile-time bundled extensions, the same `Decoder` impl is linked statically with zero indirection.

The performance equivalence requirement (FR-003a: "identical per-call performance") is validated by comparing `cargo bench` results between the FFmpeg extension loaded at runtime vs. statically compiled — they must differ by <1% in decode throughput.

**Alternatives considered**:
- `abi_stable` crate — provides a stable ABI layer for trait objects across shared library boundaries, but introduces `dyn Trait` dispatch overhead that violates FR-003a.
- WASM extensions — sandboxing benefits but 2-5x performance penalty for audio decoding, unacceptable for our targets.

## Decision: KDL Configuration Parsing

**Decision**: Use `kdl-rs` for KDL parsing.

**Rationale**: `kdl-rs` provides manual KDL document tree traversal. This enables strict validation of core configuration keys while allowing the `[extensions]` node's children to be passed through to extensions without core-side validation. This resolves the conflict between FR-014 (unknown key rejection) and FR-007 (extension-declared config keys).

**Alternatives considered**:
- `knuffel` — derive-macro-based decoding with compile-time schema validation. Rejected because its compile-time model cannot accommodate runtime-loaded extensions declaring their own configuration keys. Suitable for projects without plugin-config needs.

## Decision: IPC Mechanism

**Decision**: Unix domain sockets (Linux/macOS) and named pipes (Windows), abstracted behind a `Connection` trait using `tokio` for async I/O.

**Rationale**: Local IPC is required by FR-004/FR-006 (foreground playback, separate control commands). Unix domain sockets are the standard zero-copy IPC mechanism on POSIX; named pipes provide equivalent functionality on Windows. `tokio` is chosen for its mature Unix socket and named pipe support, and because it will be needed for future HTTP/gRPC protocol directives (A3).

**Alternatives considered**:
- Shared memory with `shmem` crate — lowest latency but highest complexity; over-engineered for command-level IPC where a short JSON/protobuf message per interaction is the expected workload.
- `nng` (nanomsg-next-gen) — excellent message-passing library but requires C FFI. Avoided per dependency policy (pure Rust preference).
- Raw `std::os::unix::net` — synchronous I/O blocks the engine thread waiting for commands. Rejected because it complicates the threading model.

## Decision: FFmpeg Codec Backend

**Decision**: Direct C FFI bindings to libavcodec/libavformat via `ffmpeg-sys-next`, wrapped in the `around-codec-ffmpeg` crate.

**Rationale**: FFmpeg provides the widest codec coverage (MP3, AAC, Opus, FLAC, Vorbis, WMA, ALAC) through a single library. Direct FFI minimizes overhead compared to wrapper crates that add allocation and copying. The `ffmpeg-sys-next` crate provides generated bindings; our wrapper crate adds the `Decoder` trait implementation.

**Alternatives considered**:
- Pure-Rust decoders per format (`mp3-decoder`, `opus-decoder`, etc.) — no C dependency, but requires integrating 6+ separate crates with inconsistent APIs. More maintenance burden, not less.
- GStreamer — full multimedia framework but too heavy (dependency tree >200 crates, runtime overhead). Violates A4 binary size and memory targets.

## Decision: Async Runtime

**Decision**: `tokio` with `current_thread` runtime, multi-thread disabled. Only the IPC listener uses async; the audio pipeline stays synchronous.

**Rationale**: The async runtime is needed solely for the IPC socket listener. The audio pipeline itself is synchronous (a tight real-time decode → output loop on a dedicated OS thread, not tokio). Using `tokio` with `current_thread` keeps overhead minimal (<2MB additional RSS). The same runtime will be reused for future HTTP/gRPC servers (A3).

**Alternatives considered**:
- `smol` — lighter-weight alternative but smaller ecosystem. Rejected because tokio is more battle-tested for Unix socket support.
- Raw `std` threads with blocking I/O — simplest but complicates client reconnection and multi-command concurrency. Acceptable for a CLI that accepts one command at a time, but would need rewriting for future WebSocket support.
