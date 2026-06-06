# ADR 0004: Unified Extension System

Every `.so` exporting `AROUND_META` is an Extension.  There is no
architectural distinction between "SDK," "plugin," "singleton," or
"Slot implementation" — they differ only in which **optional export
symbols** they provide.  `around-extensions` provides the host‑agnostic
loading framework: discovery, dependency resolution, lifecycle
management, and progressive‑trust crash isolation.  Every extension
trait uses `stabby` for ABI‑stable dispatch — a single indirect jump
per call.

`around-extensions` is a **library crate**, not a standalone process.
Binary crates (CLI, GUI, Web‑UI server) link it, create their own
`Framework`, and wire loaded extensions into their domain‑specific
registries (e.g. `CodecRegister` → `CodecRegistry`).  The framework
itself knows no concrete Slot types — it only sees `RegisterVTable`.

## Status

Supersedes ADR-0002 — the fn‑pointer vtable approach (`AudioStreamVTable`,
`CodecInfo`, `pub static INFO`) is replaced by stabby trait objects
(`#[stabby::stabby] trait Codec`, `DynCodecRef`, `{slot_name}_create`
registration functions).  The dispatch cost is identical (one indirect
jump); the change is from hand‑written vtables to stabby‑generated ones,
and from compile‑time static registration to runtime registration
functions necessitated by stabby's non‑const vtable construction.

Accepted

**Constitution §III deviations.**  Two deliberate trade‑offs from the
constitution's literal text:

1. *Registration model.*  The constitution specifies `pub static INFO:
CodecInfo` with the host calling `registry.push(info)`.  stabby's
trait‑object construction is not const‑fn, making static vtable
assembly impossible at compile time.  Registration functions
(`{slot_name}_create`) are runtime equivalents — the host still
controls the registration loop; only the timing of vtable construction
shifts from compile time to `dlopen` time.

2. *Compile‑time shim.*  The constitution requires a compile‑time shim
to eliminate virtual dispatch for statically‑linked extensions.  stabby
vtables are `#[repr(C)]` fn‑pointer structs — one indirect jump per
call, identical to the constitution‑approved `AudioStreamVTable`
pattern.  The "shim" is the vtable itself, compiled into the binary.
For extensions linked at compile time, the Register holds a `DynCodecRef`
constructed via `Box::new(…).into()` at host startup — the dispatch
cost is identical to runtime‑loaded extensions.

## Context

The constitution (Principle III) mandates:

> All extension types — decoders, sources, DSP effects, and output
> sinks — MUST be loadable, unloadable, and replaceable at runtime
> without interrupting active playback.

## Decisions

### 1. Extension type catalogue

Every `.so` is an Extension.  The table lists the traits defined by
each SDK crate and the most common export‑symbol patterns.  Neither the
framework nor the crate system enforces these patterns — they are
documentation of convention.

**Legend:** S = exports `AROUND_SLOTS` (defines new Register), E =
exports a `{slot_name}_create` registration function (implements a Slot),
I = exports `around_init` / `around_deinit` (lifecycle hooks).

| SDK crate | trait / type | typical exports | notes |
|---|---|---|---|
| `around-audio-sdk` | Codec | E, I | probe‑based factory |
| `around-audio-sdk` | Filter | E, I | format‑negotiated factory |
| `around-audio-sdk` | Effect | E, I | transition select |
| `around-audio-sdk` | Source | E, I | URI‑scheme factory |
| `around-audio-sdk` | Sink | E, I | Router selection |
| `around-net-sdk` | Protocol | E, I | Engine IPC handler |
| `around-backend-sdk` | BackendApi | I | multi‑connection, stabby trait |
| `around-discovery-sdk` | Discovery | E, I | search aggregation |
| `around-recommend-sdk` | Recommendation | E, I | depends on telemetry-sdk |
| `around-telemetry-sdk` | Telemetry | E, I | feedback for recommendation |
| `around-analysis-sdk` | Analysis | E, I | fingerprint / BPM |
| `around-social-sdk` | Social | E, I | |
| `around-social-sdk` | Collaborative | E, I | |
| `around-social-sdk` | LiveSync | E, I | |
| `around-social-sdk` | Community | E, I | |
| `around-storage-sdk` | Storage | I | pure‑fn API; self‑prefixed keys |
| `around-infra-sdk` | DI | I | pure‑fn API |
| `around-infra-sdk` | EventBus | I | pure‑fn API |
| `around-extension-sdk` | ExtensionManager | S, I | extension sources + management |
| `around-ui-sdk` | Widget | E, I | |
| `around-ui-sdk` | Panel | E, I | |
| `around-ui-sdk` | Theme | E, I | |

Extensions that export `AROUND_SLOTS` distribute their own SDK crates
alongside the `.so` so that consuming extensions can compile against the
trait definitions.  Extensions that export a registration function
(`{slot_name}_create`) register implementations into a Slot whose
Register was created by a prior extension (or the host).

`AROUND_ENTRIES` (a `pub static Slice<'static, EntryDef>`) was the
original pull‑model symbol for registering implementations.  It is **not
used**: stabby trait‑object construction (`DynRef::from`) is not
const‑fn, so the vtable cannot be assembled at compile time for a
`static` initializer.  Registration functions (`{slot_name}_create`)
replace it — they are called at runtime after `dlopen`, avoiding all
const‑init constraints.

### 2. `#[slot]` macro

The `#[slot]` proc‑macro marks a `#[stabby::stabby]` trait and generates:

- **`const NAME: &'static str`** — globally‑unique identifier
  (`"around-audio-sdk::Codec"`), derived from crate path and trait name.
- **`#[repr(C)] RegisterVTable`** — two fn‑pointer fields (§6).
- **`struct {Trait}Register`** — concrete storage; layout controlled by
  `storage`.
- **`unsafe fn from_raw(vtable, instance)`** — cast helper.
- **`impl RegisterVTable` constants** — `static` vtable with generated
  `push_raw` / `remove_by_meta`.

`unsafe` blocks in generated code inherit the macro's safety contract
(documented once at macro definition).  Hand‑written `unsafe` (e.g. in
a `#[slot(manual)]` Register) must carry per‑site safety comments.

**Parameters:**

| parameter | generates |
|---|---|
| `(default)` | Vec‑based Register, `for_each_entry()`, `push()`, `all_entries()` |
| `storage = "vec"` | same as default |
| `storage = "map"` | HashMap‑based Register keyed by `*const ExtensionMeta` |
| `manual` | only `NAME` + `RegisterVTable` + `from_raw` — SDK hand‑writes Register |

### 3. SDK crate hierarchy

SDKs expose init functions called by the host at startup.  Extensions
depend on SDK crates — they do not need to depend on `around-engine`.

```rust
// SDK author writes:
pub fn init_codec() -> &'static CodecRegister {
    static REG: Lazy<CodecRegister> = Lazy::new(CodecRegister::new);
    around_extensions::attach_register(Codec::NAME, &REG.vt, &*REG as *const () as *mut ());
    &*REG
}
```

**`attach_register`** (Rust API, compile‑time Slot registration):

```rust
pub fn attach_register(
    name: &'static str,
    vtable: &'static RegisterVTable,
    reg_instance: *mut (),
);
```

SDKs may expose a feature `dynamic`, making the SDK crate itself a
loadable `.so` exporting `AROUND_META` and the optional symbols in §5.

### 4. Dispatch

Every extension trait uses `#[stabby::stabby]` — `#[repr(C)]` vtable,
`extern "C" fn` pointers, one indirect jump per call.  The stabby vtable
is a `#[repr(C)]` struct of `extern "C" fn` pointers; dispatch through
it is exactly one indirect jump — identical cost to a hand‑written C
vtable.

**Trait objects.**  The `dynptr!` macro produces ABI‑stable fat
pointers.  Never write bare `Dyn<...>`.

For the Codec slot (V0 concrete example), the owning trait object is:

```rust
pub type DynCodecRef = dynptr!(Box<dyn Codec + Send + Sync>);
// → Dyn<'static, Box<()>, vtable!(Codec)> — owns the codec definition
```

The Codec trait is stateless: all per‑stream state lives in the
opaque `stream: *mut c_void` handle allocated by `open()`.  Methods
take `&self` (the vtable pointer + a pointer to immutable codec data,
e.g. compile‑time config) and the stream handle.  Multiple streams
can share the same `DynCodecRef` without contention — dispatch is a
fat‑pointer copy per stream.

In the engine, each stream's `DynamicState` holds a borrow of the
`DynCodecRef` (via `&'static DynCodecRef`, whose lifetime is guarded
by the `Arc<Library>` in `DynamicState`).  No `Arc<DynCodecRef>`
needed — the codec entry lives in the Register, the Register lives
in the framework's `slot_map`, and `remove_by_meta` waits for all
streams to finish before removing.

### 4.1 Rust compiler version constraint

`#[stabby::stabby]` trait vtable construction depends on the
`IConstConstructor` trait in stabby‑abi.  On Rust stable ≥1.78, this
trait's implementation is gated behind `#[cfg(feature = "test")]` in
stabby‑abi.  Without it, the vtable cannot be constructed — the trait
body resolves to `compile_error!(...)`.

The `test` feature name is misleading: it is a regular Cargo feature
(not `#[cfg(test)]`).  It enables the correct code path for
`IConstConstructor` on modern stable Rust.  It is safe for production use.

All crates using `#[stabby::stabby]` traits must declare:
```toml
[dependencies]
stabby-abi = { features = ["std", "test"] }
```

### 4.2 Persistent dispatch vs transient callbacks

Extension traits use `#[stabby::stabby]` for persistent dispatch —
methods called across the `.so` boundary throughout a stream's lifetime
(`read`, `seek`, `drop`).  These require ABI‑stable vtables and stabby's
`dynptr!` trait objects.

For transient callbacks — function pointers passed as arguments to a
single `open()` call and never retained — raw `extern "C"` function
pointers are used.  These are inherently ABI‑stable (they are the
foundation stabby builds on) and avoid the overhead of a full stabby
trait implementation for single‑use callbacks.

Example: passing a reader to `Codec::open()`:
```rust
pub type ReadFn = unsafe extern "C" fn(ctx: *mut c_void, buf: *mut u8, len: usize) -> i64;
pub type SeekFn = unsafe extern "C" fn(ctx: *mut c_void, pos: i64, whence: i32) -> i64;
```

### 4.3 Codec trait (V0 reference)

```rust
#[stabby::stabby]
pub trait Codec {
    fn probe(&self, header: Slice<'_, [u8]>, filename: Slice<'_, [u8]>) -> u8;
    fn name(&self) -> StabbyString;
    fn open(
        &self,
        reader_ctx: *mut c_void,
        reader_read: ReadFn,
        reader_seek: SeekFn,
        out_stream: *mut *mut c_void,
    ) -> i32;  // 0 = success, negative = error code
    fn read(&self, stream: *mut c_void, buf: SliceMut<'_, [f32]>) -> i32;
    fn seek(&self, stream: *mut c_void, frame: u64) -> i64;
    fn drop(&self, stream: *mut c_void);
}
```

- `probe` returns 0–100 confidence; 0 = no match.  `header` is the
  initial file buffer; `filename` is the original filename.  The host
  may retry with a larger buffer (mirroring FFmpeg's `AVPROBE_SCORE_RETRY`
  pattern) but the probe function itself never accesses the stream.
- The host provides a fixed initial buffer (≥ 2048 bytes, matching
  FFmpeg's `PROBE_BUF_MIN`).  The probe function operates solely on
  this buffer; if a format requires more data, the host may retry with a
  larger buffer (up to 1 MiB).
- `name` returns the extension name used for candidate filtering (e.g.
  `"flac"`).  No separate static export (`around_codec_NAME`) — the
  trait method is the sole source.
- `open` receives transient reader callbacks (`reader_ctx`, `reader_read`,
  `reader_seek`) valid only during the call.  Returns a stream handle
  via `out_stream` and an error code.  Audio metadata (sample rate,
  channels, total frames) is read by the caller from the container format
  — it is not part of the Codec trait.
- `stream` is an opaque pointer allocated by `open`.
- `Slice` / `SliceMut` are stabby's ABI‑stable slice types, replacing
  raw `(ptr, len)` pairs for non‑opaque parameters.

### 5. Extension loading and dependency resolution

Extensions load lazily and recursively.  `Framework::load()` calls
itself for each dependency, so a dependency's `around_init` runs before
the dependent's — dependencies must be ready before dependents.
#### 5.1 Index

`Framework::scan()` populates an extension index — a `HashMap<name, (path,
ExtensionMeta)>` — by walking search paths and calling `dlsym` for
`AROUND_META` on each `.so`.  No full load (`around_init`, `_create`)
is performed.  The index is cheap (metadata only) and enables fast
dependency lookup.

Extension sources, in priority order:

1. **Extension index** — pre‑scanned local files.  First lookup is O(1).
2. **Fresh scan** — targeted search of configured directories for a
   specific extension by name (a single pass, not a full index rebuild).
3. **Remote registry** — user‑configured URL (e.g. an npm‑style package
   index).  Downloaded `.so` is verified, cached, then loaded.

A dependency is satisfied if any source provides a matching extension
name.  `load()` fails only after exhausting all three.

#### 5.2 Pull‑model entry points

```rust
#[no_mangle] pub static AROUND_META: ExtensionMeta = ...;                     // required

#[no_mangle] pub static AROUND_SLOTS: stabby::Slice<'static, SlotDef> = ...;   // optional

#[no_mangle] pub extern "C" fn around_init() { ... }                          // optional
#[no_mangle] pub extern "C" fn around_deinit() { ... }                        // optional

// Registration function: called by host after dlopen.
// Index-based: caller polls with incrementing index until null is returned.
#[no_mangle]
pub unsafe extern "C" fn around_codec_create(index: usize) -> *mut c_void {
    match index {
        0 => {
            let codec: DynCodecRef = Box::new(MyFlacCodec).into();
            Box::into_raw(Box::new(codec)) as *mut c_void
        }
        _ => std::ptr::null_mut(),
    }
}
```

`AROUND_META` is the single required export — identity, version, and
dependencies.  `AROUND_SLOTS` defines new Slot Registers for other
extensions to populate.  Registration functions (named `{slot_name}_create`,
e.g. `around_codec_create`) are the pull‑model mechanism for providing
Slot implementations.

```rust
#[repr(C)]
pub struct SlotDef {
    pub slot_name: &'static str,
    pub reg_vtable: *const (),
    pub reg_instance: *mut (),
}
```

**Why `AROUND_ENTRIES` is dead.**  `DynRef::from()` is not const‑fn,
`DynRef` is not `Sync`, and `LazyLock` cannot cross FFI boundaries.
Registration functions avoid all three.

**Multi‑implementation support.**  The index parameter lets one `.so`
register multiple implementations of the same Slot.  The host polls
`create(0)`, `create(1)`, … until null — no separate `count` function.

Error codes from `open()` follow the convention `-1` = generic failure,
`-2` = corrupt data, `-3` = unsupported format variant, `-4` = out of
memory.  `read()` and `seek()` return negative codes on error (same
convention); 0 or positive on success.

#### 5.3 Recursive `load()`

```
Framework::load(name_or_path):
  // 1. Resolve the extension identity
  path = index.lookup(name)
      ?? fresh_scan(name)
      ?? (remote_configured ? remote_download(name) : null)
  if !path: FAIL "unresolved: <name>"

  // 2. dlopen + read identity
  lib = dlopen(path, RTLD_LOCAL)
  meta = dlsym(lib, "AROUND_META")

  // 3. Resolve dependencies — recursive, depth-first
  loading_set.insert(meta.name)              // guard against cycles
  for dep_name in meta.depends_on:
      if dep_name in loading_set:            FAIL "circular dependency: <dep_name>"
      if dep_name is already loaded:         continue            // satisfied
      if dep_name is in index:               load(dep_name)      // recurse
      else:                                  // try fresh scan → remote
          dep_path = fresh_scan(dep_name)
              ?? (remote_configured ? remote_download(dep_name) : null)
          if !dep_path: FAIL "unresolved dependency: <dep_name>"
          index.insert(dep_name, dep_path, …)                    // cache for future lookups
          load(dep_path)
  loading_set.remove(meta.name)

  // 4. All dependencies are now loaded and initialised.  Initialise self.
  dlsym(lib, "AROUND_SLOTS")     → attach_register()          // define new Slots (optional)
  dlsym(lib, "around_init")      → call                       // self‑initialisation (optional)
  for each Slot in slot_map:
      dlsym(lib, "{slot}_create") → poll index 0,1,… until null
          → push_raw(slot, ptr, meta)                          // register entries

  push LoadedExtension { meta, path, lib }
  broadcast OnLoad(meta)
```

Recursion guarantees topological order: a dependency's `around_init` runs
before the dependent's `around_init`.  The depth‑first recursion
ensures this naturally — no separate `sort_by_deps()` phase.  The
`loading_set` (a transient `HashSet<Box<str>>` created per top‑level
`load()`) detects and rejects circular dependencies before recursion.
Remote download is gated behind a user‑configured registry URL —
without it, only local sources (index + fresh scan) are consulted.

The `Framework` Mutex (§6) is held during the top‑level `load()` but
released before each recursive call into a dependency — the
pseudocode's `load(dep_name)` is a private `load_locked(&mut
FrameworkInner, …)` that operates on the already‑acquired lock.  This
prevents deadlock on re‑entrant mutex acquisition.
#### 5.4 Lazy loading

Start‑up only calls `Framework::scan()` to build the index — no
`dlopen` beyond metadata reads.  Full `load()` is triggered by:

- **Explicit request** — IPC `load_extension` command, config
  `preload_extensions`, or `Engine::play()` encountering an unknown format
  whose file extension matches a lazy index entry's codec name.  (The
  index records only identity and dependency metadata — full
  `probe()`‑based format detection requires loading the extension first.)
- **Dependency chain** — when a requested extension declares
  `depends_on`, each dependency is loaded recursively before the
  dependent is initialised.

The index's metadata (`ExtensionMeta`) includes `depends_on` and
`depends_on_slots`, so dependency resolution can be performed entirely
from index data — no full load needed to check whether dependencies are
satisfiable.

#### 5.5 C ABI exports (for loaded extensions)

Every Rust API has a corresponding C ABI symbol:

```rust
// ── lifecycle ──
pub unsafe extern "C" fn around_load(path: StabbyString) -> *const ExtensionMeta;
pub unsafe extern "C" fn around_load_bytes(bytes: &[u8]) -> *const ExtensionMeta;
pub unsafe extern "C" fn around_unload(meta: *const ExtensionMeta) -> bool;
pub unsafe extern "C" fn around_prepare_reload(path: StabbyString) -> *mut c_void;
pub unsafe extern "C" fn around_commit_reload(old_meta: *const ExtensionMeta, pending: *mut c_void) -> bool;

// ── Slot management ──
pub unsafe extern "C" fn around_attach_register(
    slot_name: StabbyString, vtable: *const (), reg_instance: *mut ()
);
pub unsafe extern "C" fn around_push_entry(
    slot_name: StabbyString, vtable: *const (), entry: *mut (), source: *const ExtensionMeta
);

// ── dependency graph ──
pub unsafe extern "C" fn around_dependencies_of(meta: *const ExtensionMeta) -> StabbySlice<*const ExtensionMeta>;
pub unsafe extern "C" fn around_is_loaded(name: StabbyString) -> bool;
```

`around_attach_register` registers a new Slot into the Framework's slot
map.  `around_push_entry` dispatches through `RegisterVTable::push_raw`
(§6), enabling runtime implementations to register without recompilation.

Three paths for Slot registration:

| path | API | when |
|---|---|---|
| Compile‑time | `attach_register` (Rust) | host startup |
| `.so` load | `static AROUND_SLOTS` | `load()` |
| Runtime | `dlsym("around_attach_register")` | post‑load |

### 6. `around-extensions` internals

`around-extensions` is the generic extension framework.  It knows **no
concrete Slot types** — `Codec`, `Filter`, `Source`, etc. live in their
respective SDK crates.  The framework only operates on `RegisterVTable`
pointers and opaque entry pointers.  Concrete entry storage (e.g. a
`Vec<DynCodecRef>` for codecs) is internal to each SDK's Register
implementation, invisible to the framework.

```rust
#[repr(C)]
pub struct RegisterVTable {
    pub push_raw: unsafe extern "C" fn(*const (), *const (), *mut (), *const ExtensionMeta),
    pub remove_by_meta: unsafe extern "C" fn(*const (), *const ExtensionMeta),
}

struct Framework { inner: Mutex<FrameworkInner> }
struct FrameworkInner {
    slot_map: HashMap<&'static str, SlotStorage>,
    loaded: Vec<LoadedExtension>,
    pending: Vec<PendingEntry>,
    index: HashMap<Box<str>, (PathBuf, ExtensionMeta)>,  // name → (path, metadata); from scan()
    on_load: Box<OnLoadRegister>,
    on_unload: Box<OnUnloadRegister>,
    on_reload: Box<OnReloadRegister>,
}

struct PendingEntry {
    slot_name: &'static str,
    vtable: *const (),
    instance: *mut (),
    source_meta: *const ExtensionMeta,
}

// `PendingEntry` buffers entries discovered during dependency resolution
// when an entry's target Slot Register has not yet been created.  Once
// the providing extension loads and attaches the Register, buffered
// entries are drained into it.  This handles the case where an
// implementing `.so` loads before the SDK `.so` that defines the Slot.

enum SlotStorage {
    Static  { register: &'static RegisterVTable, reg_instance: *mut () },
    Owned   { register: &'static RegisterVTable, reg_instance: *mut (),
              source_meta: *const ExtensionMeta },
}

struct LoadedExtension { meta: *const ExtensionMeta, path: PathBuf, _lib: Library }
```

`RegisterVTable` dispatches into per-Register storage.  Concrete
Register implementations (e.g. `CodecRegister`) store entries in their
own internal `Vec<T>` where `T` is the slot‑specific entry type (e.g.
`DynCodecRef` for Codec).  Query, probe, and iteration methods are
provided by the Register, not the framework.

Per‑Register: `RwLock<Vec<Entry>>` or `RwLock<HashMap<...>>`.
Query = read lock.  push/remove = write lock.  Framework Mutex
serialises load/unload sessions — during load/unload, all Register
queries are excluded (the Mutex is held).  Hot‑path decode does not
acquire the Mutex; it only accesses stream state and codec vtables
already loaded.  Fine for V0; migrates to lock‑free in H1.

#### 6.1 Slot access patterns

`SlotStorage::Static` — Register created at compile time by the host
binary (e.g. `init_codec()` in `main()`).  The Register's static
reference is returned by the init function and used by SDK consumers
for queries.

`SlotStorage::Owned` — Register created by a loaded `.so` exporting
`AROUND_SLOTS`.  The Register's vtable and instance pointer come from
the `.so`; entries pushed into it may come from other extensions.

**unload() contract:**

Streams that hold references into a loaded extension (via
`DynamicState::_library: Arc<Library>`) prevent `dlclose` until the
last stream drains.  The unload contract does not need an explicit
wait‑for‑streams step: the `Arc<Library>` clone held by each stream's
`DynamicState` ensures the `.so` remains mapped until all streams drop
their handles.  `remove_by_meta` only removes the Register entry; the
`DynCodecRef` stays reachable via stream borrows until the `Arc<Library>`
reaches zero.

1. Broadcast `OnUnload(ext.meta)` with `catch_unwind`.
2. All Registers: `remove_by_meta(ext.meta)`.
3. Remove `SlotStorage::Owned` where `source_meta == ext.meta`.
4. Call `around_deinit` — extension tears down its own state.
5. Drop `LoadedExtension` → dlclose.

### 7. Lifecycle hooks

| Slot | Register | when | context |
|---|---|---|---|
| `OnLoad` | Vec | after any extension loads | `meta` of loaded extension |
| `OnUnload` | Vec | before any extension unloads | `meta` of unloading extension |
| `OnReload` | Vec | prepare phase of any reload | `meta` of old extension |

`Framework::new()` constructs the three lifecycle Registers.  All
dispatchers wrap each listener in `catch_unwind`.  Nested `load()`
during `around_init`/`around_deinit` prohibited.

#### Example: Engine quiesce via OnReload

```rust
fn on_reload(old_meta: &ExtensionMeta) {
    for id in engine.streams().filter(|s| s.uses_extension(old_meta)) {
        engine.stream(id).pause();
        engine.stream(id).wait_drain();   // drain in-flight read() only;
                                          // ring buffer still has audio → no gap
        engine.stream(id).swap_vtable();
        engine.stream(id).resume();
    }
}
```

> **Requires ADR‑0005** for `Stream` / `StreamId` types and per‑stream
> pause/ring‑buffer primitives.

### 8. Progressive trust

Extensions load with "untrusted" status.

**around_init / around_deinit / push_raw:** called once per extension.
Always wrapped in a crash guard.

**push_raw:** called per registered implementation — may be called many
times across multiple `.so` loads.  First N invocations guarded per
Register; guard removed after N successes.

**Hot‑path Codec / Filter:** host‑level progressive trust.  Engine's
decode loop wraps first N `read()`/`process()` per extension in a
conditional‑branch guard; guard elided after N successes.  Framework
provides trust metadata only.

### 9. Hot‑reload

1. `prepare_reload(new_path)`: verify file signature → dlopen → read
   symbols → buffer registrations → return `PendingReload`.

2. `commit_reload(ext, pending)` under `inner.lock()`:
   a. Broadcast `OnReload(old_meta)` — Engine quiesces.
   b. Broadcast `OnUnload(old_meta)`.
   c. `remove_by_meta(old_meta)`.
   d. Remove Owned Slots.
   e. `around_deinit` old.
   f. Drop old `LoadedExtension` → dlclose.
   g. Commit buffered → live slots.
   h. Push new `LoadedExtension`.
   i. `around_init` new.
   j. Broadcast `OnLoad(new_meta)`.

### 10. Extension metadata

```rust
#[stabby::stabby]
pub struct ExtensionMeta {
    pub name: &'static str,
    pub semver: &'static str,
    pub api_version: u32,
    pub depends_on: &'static [&'static str],
    pub depends_on_slots: &'static [&'static str],
    pub author: &'static str,
    pub description: &'static str,
}
```

`depends_on` — extensions by name.  Topological sort.
`depends_on_slots` — Slots by symbolic NAME.  Deferred to `pending`.

All string fields use `&'static str` — these are compile‑time string
literals, fully const‑constructible in a `static`.  `#[stabby::stabby]`
maps `&'static str` to `#[repr(C)] { ptr: *const u8, len: usize }`,
giving native FFI stability without heap allocation.

### 11. Cross‑extension dynamic linking

Extensions use standard OS dynamic linking.  Provider `.so` exports
symbols; consumer `.so` links against the provider at build time; the
OS loader resolves at runtime.  No `RTLD_GLOBAL`, no
`--allow-shlib-undefined`.

**Split‑crate architecture.**  Each SDK is two crates:

- `around-storage-sdk` (rlib) — `extern "C"` declarations + safe
  wrappers.  Zero object code.  Consumers depend on this.
- `around-storage` (cdylib) — implementations.  Depends on the SDK
  rlib for types.  `around-extensions` loads this at runtime.

The split prevents Cargo feature unification from injecting
implementation into consumer builds.

| Platform | Provider build.rs | Consumer build.rs |
|---|---|---|
| Linux | `rustc-cdylib-link-arg=-Wl,-soname,libaround_storage.so` | `rustc-link-search` + `rustc-link-lib=around_storage` |
| macOS | `rustc-cdylib-link-arg=-Wl,-install_name,@loader_path/libaround_storage.dylib` | same as Linux |
| Windows | `/IMPLIB` flag → `around_storage.dll.lib` | `rustc-link-search` + `rustc-link-lib=around_storage` |

The consumer links against the provider (DT_NEEDED / LC_LOAD_DYLIB /
import table).  At runtime the OS loader finds the provider by name —
because `around-extensions` already loaded it, the loader reuses the
in‑memory module.  Provider symbols resolve through the export table,
not the global symbol table — deterministic, no collision risk.

**`load_bytes` and DT_NEEDED.**  When a provider is loaded from bytes
(temp file with random path), DT_NEEDED resolution still works correctly.
The dynamic linker registers the library under its logical name
(DT_SONAME on Linux, LC_ID_DYLIB on macOS), not its filesystem path.
Consumer DT_NEEDED matches the in‑memory reverse‑map — no filesystem
lookup.  The temp file's random name is irrelevant.  This requires the
provider to set `-Wl,-soname` (Linux) / `-Wl,-install_name` (macOS)
as specified in the build table above.

All extensions are loaded with `RTLD_LOCAL`.  No `RTLD_GLOBAL`.
Independent signature verification allows the extension author to
guarantee that symbols do not conflict.

**Naming convention:** `{crate}_{fn}` — `around_storage_get`,
`around_di_resolve`, `around_event_subscribe`.

**Symbol re‑lookup.**  The host stores the `Library` handle in
`LoadedExtension._lib`.  At any point (e.g., OnLoad handlers,
post‑init queries) the host may resolve additional exported symbols
from a loaded extension via `dlsym` on this handle.  Symbols follow
the `{crate}_{fn}` convention.  This enables runtime capability
discovery, config value queries, and version‑specific feature probes
without requiring all symbols to be known at `load()` time.

### 12. Discovery

`Framework::scan()` walks search paths (`$HOME/.local/share/around/extensions/`,
`/usr/lib/around/extensions/`, `/usr/local/lib/around/extensions/`, plus
KDL config and IPC‑registered paths).  For each `.so` file it calls
`dlopen` + `dlsym("AROUND_META")` to read metadata, then immediately
`dlclose` — no `around_init`, no `_create` calls, no Slot population.
The result is an index: `HashMap<name, (path, ExtensionMeta)>`.

`load()` resolves dependencies against this index first.  If a
dependency is not in the index, a fresh scan is triggered.  Remote
sources (a user‑configured registry URL) are consulted as the last
resort.  Scanning skips `.so` files lacking `AROUND_META` silently;
explicit `load` rejects them.

Crash‑safe: fork (Linux/macOS), SEH (Windows).

### 13. around‑engine shim crate (future)

`around-engine-shim` wraps Engine functionality in stabby traits for
direct ABI‑stable access, eliminating IPC overhead.  Independent of
`around-engine`.

### 14. GUI and binary crate architecture

The GUI bin crate links `around-engine` as a library.  It can start an
in‑process Engine, connect to a remote one, or control multiple
instances.  It loads its own Slots via `Framework`.  GUI extensions
depend on SDK crates, not on `around-engine`.  `around-engine` does not
know about non‑pipeline Slots.

### 15. WASM runtime

Separate ADR, the runtime is loaded as an extension.

## Consequences

- `around-extensions` depends on `stabby` + `libloading`, never on
  any `around‑*` crate — it is a self‑contained generic framework.
- `stabby-abi` dependency with `features = ["std", "test"]` required
  for Rust stable ≥ 1.78.
- All extension traits `#[stabby::stabby]` — 1 indirect jump.
- `DynCodecRef` is owning (`dynptr!(Box<dyn Codec + Send + Sync>)`) —
  Drop frees the codec instance.
- Register stores opaque `(*const (), *mut ())` pairs; concrete entry
  types are internal to each SDK's Register.
- Registration function (`{slot_name}_create`) with index‑based polling
  replaces static `AROUND_ENTRIES` — avoids const‑init / stability issues
  and supports multiple implementations per `.so`.
- All metadata structs (`ExtensionMeta`, `SlotDef`) use `&'static str`
  for string fields — zero heap allocation in statics, fully
  const‑constructible.
- Recursive `load()` resolves dependencies depth‑first — topological order
  is guaranteed by recursion; `loading_set` detects circular dependencies.
- `scan()` builds an index of names → (path, metadata) without full load —
  start‑up touch is O(files on disk), not O(loaded extensions).
- Lazy loading: full load (dlopen + around_init + {slot}_create) deferred
  until an extension's functionality is requested or it is a dependency of
  a requested extension.
- Multi‑source dependency resolution: index → fresh scan → remote
  registry (gated behind user‑configured URL); load fails only after
  exhausting all sources.
- Pull‑model: statics + `around_init`/`deinit` + registration functions.
  Lifecycle hooks (`OnLoad`, `OnUnload`, `OnReload`) are the only
  callback mechanism — they fire at well‑defined points, not during
  arbitrary extension operations.
- `#[slot]` generates `RegisterVTable` + convenience methods.
  Hand‑written `unsafe` carries per‑site comments; macro‑generated
  `unsafe` inherits macro's contract.
- Framework Mutex serialises load/unload; per‑Register `RwLock` for
  queries.  Fine for V0; migrates to lock‑free in H1.
- `catch_unwind` wraps lifecycle broadcasts.
- Progressive trust guards all extension operations.
- Signature verification before dlopen.
- Full C ABI surface for extensions to manage extensions.
- Cross‑extension dynamic linking via platform‑native OS resolution
  with naming convention for symbol isolation.
- Provider SDK crates are distributed alongside the `.so` for
  compile‑time consumption.

## Open Questions

1. **Dynamic‑loading crate selection.**  The design uses `libloading`.
   Subagent source‑audit:
   - No Rust crate exposes `dlmopen`.  Raw `libc::dlmopen` on glibc
     Linux for namespace isolation (no need by using naming conventions).
   - `libloading` 0.9.0 (cross‑platform, `RTLD_LOCAL` default,
     `from_raw()` escape hatch).  402M downloads.
   - `dlopen‑rs` (pure‑Rust ELF linker) — Linux‑only, unsuitable.
   - `savefile‑abi` — serialisation ABI, not hot‑path.

   **Recommendation:** `libloading` primary; raw `libc::dlmopen` on
   glibc, wrapped via `from_raw()`.

2. **Stub‑crate ergonomics.**  Consumer extensions currently must
   depend on SDK rlib + link against provider cdylib at build time.
   §11 describes the architecture.  Build‑coordination tooling
   (stub `.so` for cross‑workspace linking, auto‑generation from
   `#[stabby::export]`) deferred to implementation.

3. **Remote extension registry.**  §5.1 mentions a user‑configured URL
   as a dependency source.  The registry protocol, caching strategy, and
   signature‑verification chain for downloaded `.so` files are deferred
   to implementation.

4. **Index persistence.**  The in‑memory index built by `scan()` is
   rebuilt on every cold start.  Persisting it to disk (e.g. sqlite,
   validated against file mtimes) is a natural H1 optimisation for
   large extension ecosystems.

5. **IPC extension management commands** — deferred to IPC layer.
6. **WASM runtime** — deferred to separate ADR.
7. **Signature verification** — GPG/minisign, compile‑time feature.
8. **Version constraints in depends_on** — name‑only for MVP.
9. **FormatSignature and magic‑byte detection.**  V0 codec selection
   uses `probe()` on a fixed initial buffer.  Probing with growing
   buffer sizes (FFmpeg `AVPROBE_SCORE_RETRY` pattern) and
   `FormatSignature` integration for static codec metadata are deferred
   to H1.

## Performance assessment

| metric | baseline | `around-extensions` | Δ |
|---|---|---|---|
| Codec `read(buf)` | 1 jump | 1 jump (stabby) | none |
| Filter `process(buf)` | same | same | none |
| `load()` 100 KiB `.so` | N/A | < 50 ms | — |
| `push_raw` | N/A | O(1) hash + RwLock write | — |
| Register query | N/A | RwLock read + raw vtable | — |
| `Framework::new()` + SDK init | < 1 ms | same | none |
| Per‑stream quiesce | N/A | [proj.] < 2 ms | no gap |
| Dir scan 100 files | N/A | < 500 ms (forked) | — |
| Sig verify 1 MiB `.so` | N/A | < 10 ms (minisign) | — |

| memory | per‑ext. | per‑entry |
|---|---|---|
| `LoadedExtension` | ~200 B | — |
| `SlotStorage::Owned` | ~50 B | — |
| Vec Register entry | — | ~100 B |
| HashMap Register entry | — | ~150 B |
