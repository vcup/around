# ADR 0003: IpcCodec Trait + IpcWire Enum over `Box<dyn IpcCodec>`

The IPC subsystem uses a concrete `IpcWire` enum to dispatch between codec
implementations rather than a `Box<dyn IpcCodec>` trait object. This preserves
`Send + Sync` for tokio task spawning while maintaining a clean codec
abstraction.

## Status

Accepted

## Context

The IPC server spawns one tokio task per transport (Unix socket, named pipe,
TCP, UDP) plus one per accepted connection. Each task must be `Send` to cross
`.await` points and `Sync` to be shared via `Arc`:

```rust
tokio::spawn(async move {
    serve(engine, state, listener, codec, ext_mgr).await
});
```

The codec is passed by value to `serve()` and cloned into each connection
handler. Two codec implementations exist, selectable at runtime based on
feature flags and connection auto-detection:

- `JsonLineCodec` — newline-delimited JSON (always available via `json-ipc`
  feature). Every message is one JSON line terminated by `\n`.
- `ProtoCodec` — varint-delimited protobuf (available via `protobuf-ipc`
  feature). Length-prefixed binary messages.

The natural Rust pattern for "one of N implementations of a trait, chosen at
runtime" is `Box<dyn Trait>`. However:

```rust
// Rejected approach:
let codec: Box<dyn IpcCodec> = if use_protobuf {
    Box::new(ProtoCodec)
} else {
    Box::new(JsonLineCodec)
};
```

`Box<dyn IpcCodec>` is `Send` only if `dyn IpcCodec + Send` is explicitly
spelled. But the `IpcCodec` trait's methods take `&mut BufReader<R>` and
`&mut W` where `R: AsyncRead + Unpin + Send`, `W: AsyncWrite + Unpin + Send`
— the trait itself has no `Send` bound on `Self`. Adding `Send` as a
supertrait would work but is viral: every implementor must be `Send`, and the
compiler verifies it at the trait level rather than at the use site.

Additionally, `Box<dyn IpcCodec>` introduces a heap allocation and vtable
indirection that, while not on the audio hot path (IPC codec calls happen once
per command, not per frame), adds complexity for no benefit — the set of
codec implementations is closed (two variants, both known at compile time).

## Decision

Use a concrete enum `IpcWire` that implements `IpcCodec`:

```rust
#[derive(Clone)]
pub(crate) enum IpcWire {
    Proto(ProtoCodec),
    Json(JsonLineCodec),
}

impl IpcCodec for IpcWire {
    async fn read_command<R: AsyncRead + Unpin + Send>(
        &self,
        reader: &mut BufReader<R>,
    ) -> io::Result<IpcCommand> {
        match self {
            IpcWire::Proto(c) => c.read_command(reader).await,
            IpcWire::Json(c) => c.read_command(reader).await,
        }
    }
    // write_response follows the same pattern
}
```

`IpcWire` is a plain data type — `Clone`, `Send`, `Sync` are all derived
automatically because both variants are. The `IpcCodec` trait remains clean
(async methods, no `Send` supertrait needed) because the `Send` requirement is
pushed to the concrete type, not the abstraction.

### Transport-agnostic trait

The `IpcCodec` trait is transport-agnostic — it operates on `AsyncRead` /
`AsyncWrite` streams, not on specific transport types. All four transports
(Unix socket, named pipe, TCP, UDP) use the same `IpcCodec` implementation
through the shared `handle_connection()` and `serve()` functions:

```rust
async fn handle_connection<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    engine: Arc<Engine>,
    state: Arc<Mutex<PlaybackState>>,
    ext_mgr: Arc<ExtensionManager>,
    codec: impl IpcCodec,  // IpcWire or any future codec
) -> io::Result<()>
```

### Codec auto-detection

At connection time, the server peeks at the first byte of the incoming stream:

- `0x0a` → protobuf (first varint byte of field 1, `PlayCommand`)
- `{` → JSON (opening brace of `{"command":"play",...}`)

This auto-detection is handled at the transport level, not in the codec trait,
and simply selects which `IpcWire` variant to construct.

## Consequences

### Positive

- `IpcWire` is `Send + Sync + Clone` automatically — no manual unsafe impls,
  no trait object Send holes.
- Zero heap allocation for the codec — `IpcWire` is stack-allocated and
  `Clone` is a trivial copy.
- The `IpcCodec` trait stays clean without `Send` supertraits.
- Match-based dispatch compiles to a simple branch, not an indirect call.
- Adding a third codec requires adding one variant to `IpcWire` and one arm
  to each match — the compiler enforces exhaustiveness.

### Negative

- The set of codec implementations is closed — adding a new codec requires
  modifying the `IpcWire` enum (in `around-engine`). This is acceptable
  because codecs are a core concern, not an extension point: there is no use
  case for third-party IPC codecs.
- The match arms in `IpcWire` are boilerplate. With only two variants this is
  negligible; if the set grows beyond 3–4, a macro or code generation should
  be considered.

## Related

- Constitution §III (Zero-Overhead Extensibility), §IV (Cross-Platform &
  Form-Factor — IPC must work across transports)
- ADR-0002 (Fn-Pointer VTable over `dyn Codec`) — same philosophy, different
  trade-off (open set of codecs vs. closed set of IPC codecs)
- `crates/around-engine/src/ipc/codec.rs` — `IpcCodec` trait, `IpcWire` enum,
  `JsonLineCodec`
- `crates/around-engine/src/ipc/codec_proto.rs` — `ProtoCodec`
- `crates/around-engine/src/ipc/mod.rs` — `serve()`, `handle_connection()`
