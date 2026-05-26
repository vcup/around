<!--
  Sync Impact Report
  ==================
  Version change: 1.7.0 → 1.8.0
  Bump rationale: MINOR — architecture constraints expanded (LAN device sync,
                   literate programming docs, ADR process).
  Modified principles: None
  Added sections: None
  Removed sections: None
  Templates requiring updates:
    ✅ .specify/templates/plan-template.md — no changes needed
    ✅ .specify/templates/spec-template.md — no changes needed
    ✅ .specify/templates/tasks-template.md — no changes needed
    ✅ .specify/templates/checklist-template.md — no changes needed
  Follow-up TODOs: None
-->

# around music player constitution

## Core Principles

### I. Format Universality

The player MUST support decoding of all common and well-documented audio formats
(MP3, FLAC, WAV, AAC, OGG Vorbis, Opus, WMA, ALAC, AIFF, and others as they
become relevant). Each format decoder MUST implement the shared `Decoder` trait
defined by the core, decoupling format-specific logic from playback and UI layers.

**Rationale**: A music player that claims to support "all imaginable formats" must
demonstrate this through a uniform, extensible decoding pipeline. Centralizing the
decoder contract prevents format-specific logic from fragmenting the codebase.

### II. Source Agnosticism

All audio sources — local filesystem paths, HTTP/HTTPS remote URLs, network
streams (Icecast, SHOUTcast), attached storage devices, and virtual file systems —
MUST be treated uniformly through the shared `Source` abstraction. Source-specific
implementation details (protocol handling, buffering, authentication) MUST NOT
leak into the playback engine, decoder chain, or user interface.

**Rationale**: Users expect to play music from any location without caring about
the underlying transport. A uniform source interface enables composability (e.g.,
streaming a remote file through a local cache layer) and simplifies higher-level
logic that should not depend on where bytes originate.

### III. Zero-Overhead Extensibility

Every format decoder, source handler, DSP effect, and output sink MUST be a
self-contained, independently compilable Rust crate. The core application MUST
remain minimal — providing only the playback engine, audio pipeline graph,
media library, and IPC backbone.

All extension points MUST enforce **zero-overhead abstraction**:

- Extension trait boundaries MUST be resolvable at compile time via static
  dispatch (no `dyn Trait` in hot paths). Use `enum_dispatch`, generics with
  monomorphization, or equivalent compile-time polymorphism.
- Runtime dynamic loading (`libloading`, `dlopen`) is permitted only for
  third-party extensions distributed as binaries. Even then, a compile-time
  shim MUST exist to eliminate virtual dispatch overhead.
- **Hot-reload**: All extension types — decoders, sources, DSP effects, and
  output sinks — MUST be loadable, unloadable, and replaceable at runtime
  without interrupting active playback. The extension boundary uses native
  shared object loading (`dylib`/`.so`/`.dll`) for maximum performance.
  Hot-reload MUST NOT introduce per-call dispatch overhead; the shim layer
  resolves symbols once at load time.
- Any abstraction layer MUST compile to the same (or fewer) instructions as
  the equivalent directly-inlined code. Extensibility MUST NOT be a performance
  tax.

Third-party extensions for proprietary or niche formats MUST be loadable and
unloadable without modifying or recompiling the core.

**Rationale**: Extensibility and performance are traditionally in tension.
Rust's trait system and zero-cost abstractions make it possible to have both.
Every cycle spent on dynamic dispatch in an audio pipeline is a cycle stolen
from decoding, DSP, or low-latency output. Static dispatch also enables
aggressive compiler optimizations (inlining, vectorization) across crate
boundaries.

### IV. Cross-Platform & Form-Factor by Design

The application MUST compile and run on Linux, macOS, and Windows. All
platform-specific code (audio output backends, file system watchers, system tray
integration, media key handling) MUST be isolated behind platform trait
abstractions and conditionally compiled with `#[cfg]` attributes. Platform
abstractions MUST be defined in the core crate and implemented per-platform in
dedicated crates.

Beyond operating systems, the application MUST support multiple form factors:
- **Headless / Server**: Operate as a daemon or background process without any
  display server dependency. Remote control via HTTP API, CLI commands, or
  network protocols (e.g., MPRIS, REST).
- **CLI / TUI**: Full-function text-based interface for terminal environments.
- **GUI Desktop**: Full windowed interface with optional compact/mini modes.

Form-factor code MUST follow the same isolation pattern as platform code —
behind traits and `#[cfg]` / feature gates — so that building a headless binary
requires no GUI dependencies.

**Rationale**: "Any location" means any operating system AND any usage context.
A server admin should control playback from SSH; a desktop user should get a
rich GUI; a tiling WM user should have a TUI. Separating form-factor logic
from the core playback engine keeps the binary size small for constrained
environments.

### V. Test-First Discipline (NON-NEGOTIABLE)

All new functionality MUST follow Test-Driven Development:

1. Write tests that define the expected behavior.
2. Verify tests fail against the current implementation.
3. Implement the minimum code to make tests pass.
4. Refactor while keeping tests green.

Contract tests for each `Decoder` and `Source` extension are mandatory —
they MUST validate that every implementation satisfies its trait contract.
Integration tests MUST cover end-to-end playback pipelines (source → decode →
output) for at least one format per decoder family.

**Rationale**: Audio playback involves complex pipelines with many failure modes
(corrupt files, network interruptions, format edge cases). TDD ensures that
every behavior is specified before implementation and that regressions are
caught immediately.

### VI. Powerful Configuration

The configuration system MUST satisfy two competing goals simultaneously:
**simplicity by default** and **extensibility without limit**.

- **Zero-config startup**: The player MUST work out of the box with sensible
  defaults. A first-time user MUST be able to play a file without touching any
  configuration.
- **Layered overrides**: Configuration MUST be read from a stack of sources
  (hardcoded defaults → system-wide config → user config → environment variables
  → CLI flags), with each layer overriding the previous.
- **Hot reload**: Configuration changes MUST take effect at runtime without a
  restart, wherever technically feasible. File-based config MUST be watched for
  changes via filesystem events.
- **Schema-driven**: The config schema MUST be formally defined (e.g., via
  `serde` + a documented struct). Unknown keys MUST be rejected with a clear
  error. Validation MUST happen at load time.
- **Extension config**: Extensions MUST be able to declare their own
  configuration namespaces. The core MUST merge and validate extension config
  transparently.

**Rationale**: A music player that works "anywhere" — from a headless server
controlled via API to a desktop GUI — needs a config system that is dead-simple
for the 90% case yet unbounded for power users. Layered overrides let the same
binary behave as a server daemon, a TUI jukebox, or a desktop mini-player
depending solely on configuration.

### VII. Unobtrusive Presence ("around")

The player MUST embody the "around" philosophy: always available, never in the
way.

- **Background-first architecture**: The playback engine MUST run as a
  long-lived background process that is independent of any specific UI. UIs
  connect to and disconnect from the engine without interrupting playback.
- **Subtle display modes**: On desktop platforms, the player MUST support
  compact overlays, mini-players, system tray integration, and "desktop widget"
  modes that present minimal visual footprint.
- **Keyboard-driven control**: Every function MUST be accessible via keyboard
  shortcuts and CLI commands. Global hotkeys (media keys) MUST be supported
  where the platform permits.
- **No forced interaction**: The player MUST NOT steal focus, show popups, or
  demand user attention except on explicit user request. Notifications MUST be
  opt-in.
- **Seamless mode transitions**: Switching between headless, CLI, TUI, and GUI
  modes MUST not interrupt playback. The current UI MUST be detachable and
  re-attachable at any time.

**Rationale**: "around" is not just a product name — it is the design
philosophy. The player should feel ambient, present in the user's environment
without demanding attention. Decoupling the engine from the UI is the
architectural foundation for this: one engine, many faces.

### VIII. Comprehensive Media Library

around MUST maintain a full-featured media database that unifies all sources
— local files, remote URLs, network streams, and attached devices — into a
single coherent collection.

- **Automatic indexing**: The library MUST scan configured directories and
  remote endpoints, extracting and storing metadata for every track.
- **Audio fingerprinting**: Acoustic fingerprints (via Chromaprint/AcoustID or
  equivalent) MUST be computed for all tracks to (a) identify untagged or
  mislabeled files and (b) detect duplicates across different formats, bitrates,
  and encodings.
- **Fuzzy rematching**: When files move or are renamed, the library MUST
  re-associate them using a combination of path heuristics, metadata matching,
  and acoustic fingerprints — not relying on fragile absolute paths alone.
- **Unified view**: Whether a track resides on the local SSD, a mounted NAS, or
  a remote HTTP endpoint, it MUST appear as an equal citizen in the library.
  Remote tracks that become unavailable MUST be clearly indicated without
  blocking the UI.
- **Smart collections**: The library MUST support rule-based smart playlists,
  tag hierarchies, ratings, play counts, skip counts, and last-played
  timestamps. Recommendations based on listening history are a goal, not a
  mandate for v1.
- **Multi-layered discovery**: Discovery MUST operate at three independent
  layers — (a) **local acoustic analysis** using extracted audio features
  (BPM, key, spectral centroid, MFCCs) for similarity clustering and offline
  radio; (b) **self-built recommendation models** trained on local play
  history using collaborative filtering and content-based approaches, running
  entirely offline; (c) **external service backends** (Last.fm, ListenBrainz,
  streaming APIs) as pluggable extensions. Users MAY contribute anonymized
  acoustic feature vectors and listening patterns to a shared analytics pool
  to improve recommendations for all users — strictly opt-in.
- **Scalability**: The library MUST perform acceptably with collections of
  100,000+ tracks. Indexing and querying MUST use efficient on-disk structures
  (SQLite or equivalent) rather than in-memory approaches.

**Rationale**: A music player that unifies "all sources" needs a brain. Without
a library, the player is merely a file opener with a remote fetcher — two
disconnected experiences. The library is the gravitational center that makes
"around" feel like one product, not a collection of features.

### IX. Protocol Diversity

The remote control surface of around MUST expose multiple protocols, each
serving a different segment of the ecosystem:

- **CLI**: A native `around` subcommand (e.g., `around play`, `around next`,
  `around status --json`) for scripting and terminal users. MUST support both
  human-readable and machine-parseable (JSON) output.
- **HTTP REST + WebSocket**: A self-hosted HTTP server providing a RESTful API
  for state query and mutation, plus a WebSocket endpoint for real-time events
  (track change, playback progress, library scan status). This enables web
  frontends and mobile companion apps.
- **gRPC / Protobuf**: A binary protocol option using protobuf schemas for
  clients that prioritize bandwidth efficiency and parsing speed over HTTP
  verbosity. The protobuf schema MUST be the canonical data model for all
  internal IPC.
- **MPRIS**: Full D-Bus MPRIS2 compliance on Linux, enabling integration with
  desktop environments, status bars, and other MPRIS-aware tools.
- **Pluggable protocol extensions**: The IPC layer MUST expose a `Protocol`
  trait so that additional control protocols (e.g., MQTT for IoT, REST for
  smart speakers) can be added as extensions.

All protocols MUST share a single backend — the same command dispatcher that
processes a "play" request from CLI also serves the REST and gRPC endpoints.

**Rationale**: "Any location" extends to any way of controlling the player. A
user on SSH needs CLI; a web dashboard needs REST; a mobile app needs efficient
binary framing; a Linux desktop expects MPRIS. Building one backend and
projecting it through many protocol facades avoids the maintenance nightmare of
parallel implementations.

### X. Extreme Performance

Performance is NOT an optimization target — it is a non-negotiable design
constraint from day one.

- **Audio pipeline latency**: End-to-end latency from source read to audio
  output MUST be under 10ms on consumer hardware. DSP chains MUST add sub-1ms
  per effect at typical settings.
- **Memory budget**: The core engine MUST consume under 50MB resident memory
  at idle (no playback, no library scan). Full playback with DSP chain MUST
  stay under 100MB. The library index MUST use memory-mapped I/O rather than
  loading datasets into RAM.
- **CPU overhead**: At idle, the engine MUST consume under 0.1% CPU on modern
  hardware. During playback with a typical DSP chain, CPU utilization MUST
  stay under 5% of a single core (excluding format decoding, which varies by
  codec).
- **Startup time**: The engine MUST reach ready state in under 500ms cold
  start, under 100ms warm start (process already resident).
- **Binary size**: The minimal headless binary (core engine + basic transports)
  MUST be under 10MB stripped. Feature-gated optional components (GUI, extended
  codecs) may increase size proportionally.
- **Benchmarking**: Every release MUST include benchmark results for the above
  metrics. Regressions beyond 10% MUST be treated as blocking bugs.

**Rationale**: A music player that runs "anywhere" includes constrained
environments — Raspberry Pi, low-power home servers, ancient laptops.
Bloat and latency are the enemies of ubiquity. Building performance in from
the start prevents the inevitable "it got slow" rewrite.

### XI. Data Resilience

around MUST treat user data as sacred. No crash, power loss, disk failure, or
user mistake should result in permanent loss of playlists, library metadata,
or playback history.

- **Crash safety**: All persistent state MUST be written atomically (write to
  temp file, fsync, rename). SQLite with WAL mode is the recommended backend.
  Corrupted state MUST be automatically detected and recovered from the last
  known-good checkpoint.
- **Automatic backups**: The library database MUST be backed up automatically
  on a configurable schedule. Playlists MUST be exportable to standard formats
  (M3U, XSPF). Backups MUST be self-contained — a single file or directory.
- **Disk change resilience**: If a monitored directory is moved, renamed, or
  remounted at a different path, the library MUST detect the change and
  re-associate tracks using metadata and acoustic fingerprints. Users MUST be
  able to remap mount points without losing their library.
- **Graceful degradation**: If a remote source becomes unreachable, tracks from
  that source MUST be marked as unavailable but NOT removed from the library.
  Playlists containing unavailable tracks MUST continue to function, skipping
  only the missing entries.
- **No silent data loss**: Any operation that would delete user data (e.g.,
  removing a library directory, clearing play history) MUST require explicit
  confirmation and provide an undo path where technically feasible.

**Rationale**: Music collections are built over decades. Losing a library
database to a crash or disk migration should be unacceptable for any serious
player. around positions itself as the user's music companion — that trust
requires treating their data with the same care as a database system.

### XII. Privacy by Design

around MUST be offline-first and privacy-respecting by default.

- **Zero network by default**: The application MUST NOT initiate any network
  connection unless explicitly configured by the user to do so (e.g., adding a
  remote music source, enabling fingerprint lookup, or activating remote
  control).
- **Opt-in telemetry**: All usage statistics, crash reporting, and feature
  analytics MUST be opt-in and disabled by default. The opt-in prompt MUST
  clearly describe what data is collected, how it is transmitted, and how to
  disable it. Telemetry payloads MUST be documented and anonymized.
- **Local-first architecture**: All core functionality (playback, library
  management, DSP, playlists) MUST function fully without internet access.
  Features that require connectivity (remote sources, fingerprint lookup, web
  UI) MUST degrade gracefully when offline.
- **No third-party trackers**: Built-in components MUST NOT include analytics
  SDKs, tracking pixels, or telemetry of any kind beyond the user-visible opt-in
  system described above.
- **Data portability**: All user-generated data (library, playlists, ratings,
  history, configuration) MUST be exportable in documented, open formats. Users
  MUST be able to leave around without losing their data.

**Rationale**: A music player has no business knowing what you listen to. Users
come from both the local-library world (privacy-sensitive) and the streaming
world (convenience-oriented). around earns trust by defaulting to local-only and
letting the user choose which bridges to the network they want to build.

### XIII. Beyond Local vs. Cloud

around MUST reject the false dichotomy between "local-library-only" and
"cloud-streaming-only" music players. Both paradigms have proven strengths, and
around MUST absorb the best of each without dogma.

- **Local paradigm strengths** (absorbed): Ownership of music files, bit-perfect
  playback, zero network dependency, no recurring cost, full privacy, instant
  seeking, no DRM, works forever regardless of service availability or licensing
  changes.
- **Cloud paradigm strengths** (absorbed): Infinite library expansion beyond
  what the user owns, algorithmic and community-driven discovery of new artists
  and genres, recommendation engines, shared playlists, social features,
  synchronized listening across devices, "radio" and "similar to" modes.
- **No dogma**: A user MUST be able to operate around as a pure local player
  with no network features enabled. Another user MUST be able to operate around
  as a discovery-first experience driven primarily by remote sources and
  recommendations. Most users will fall somewhere in between. The architecture
  MUST NOT force any usage pattern — every feature MUST degrade gracefully to
  the available sources.
- **Unified experience**: Whether a track comes from the user's SSD or a
  streaming service API, it MUST appear in the same library, the same playlists,
  the same queue, and the same playback history. The user should not have to
  think about provenance.
- **Extensible discovery backends**: Recommendations and discovery MUST be
  pluggable. Core supports optional backends (Last.fm, ListenBrainz, streaming
  APIs), each implemented as an extension. No single service is privileged.
- **Fair play**: When a streaming track is cached locally, it MUST be treated
  identically to an owned file for the duration of the cache. Cache policies
  MUST be user-configurable and transparent.

**Rationale**: The music player ecosystem is fragmented into two camps — the
"local library" camp (foobar2000, MusicBee, mpd) and the "cloud/service" camp
(Spotify, Apple Music, Tidal). Both have loyal users who would never switch
because each camp lacks what the other excels at. around's mission is to make
switching unnecessary. By absorbing both paradigms, around eliminates the
trade-off entirely: own what you love, discover what you don't yet know.

### XIV. Federated Cloud Backend

around MUST ship with an open-source, self-hostable cloud backend as part of
the same repository. This backend provides all services that commercial music
platforms offer — synchronization, social features, discovery augmentation,
shared playlists — but without platform lock-in.

- **Open source in-tree**: The backend code lives in this repository. Any user
  can deploy their own instance with a single command. No proprietary
  server-side components exist.
- **Feature parity with commercial platforms**: The backend MUST provide
  features comparable to or exceeding commercial music services: cross-device
  playback sync, shared playlists, collaborative queues, listening statistics,
  friend activity feeds, and discovery augmentation.
- **Self-hostable federation**: Instances MUST be able to discover and
  communicate with each other. A user on instance A MUST be able to share a
  playlist with a user on instance B, follow their public activity, and
  participate in cross-instance collaborative sessions. Federation MUST use
  open protocols — no instance is privileged over another.
- **Optional social identity**: Social features MUST be available without
  mandatory accounts or centralized identity providers. Users MAY operate
  purely by cryptographic identity (key pairs), shared links with capability
  tokens, or traditional username/password — the user chooses their trust
  model.
- **Practical distributed trust**: For cross-instance interactions that benefit
  from verifiable shared state (e.g., collaborative playlists with auditable
  edit history, decentralized reputation for extension registries), the backend
  MAY employ lightweight distributed ledger or content-addressable techniques.
  This is NOT a cryptocurrency — it is a practical tool for trustless
  coordination between independently operated instances.
- **Anonymous analytics pool**: Users who opt in MAY contribute anonymized
  listening patterns, acoustic features, and behavioral data to a shared
  analytics pool. This data MUST be stripped of personally identifiable
  information at the client side before upload. Aggregated insights (genre
  trends, artist similarity graphs, recommendation model weights) MUST be
  published back to all instances, creating a virtuous cycle where opt-in
  contributors improve the experience for the entire ecosystem.
- **Community not platform**: The backend is infrastructure, not a service.
  The project operates the reference instance(s) for convenience, but the
  architecture MUST assume zero trust in any specific instance. Data
  portability between instances MUST be trivial.

**Rationale**: The best music experiences today are locked inside walled
gardens. Spotify knows what you love but won't let you take it anywhere; your
local library is yours forever but lacks discovery. around's answer is not to
build a better walled garden — it is to make the garden's blueprint public,
let anyone run it, and connect them. Federation means a user's social graph
and discovery network survive any single instance going offline. Open source
means no rent-seeking on basic features.

### XV. Comprehensive Observability

around MUST provide deep visibility into every layer of the system — from the
audio pipeline to the federated backend — without degrading the performance
guarantees of Principle X.

- **Structured logging**: All components MUST emit structured, machine-parseable
  log events via `tracing`. Log levels MUST be dynamically adjustable at runtime
  without restart. Sensitive data (file paths, user metadata, authentication
  tokens) MUST be redacted from log output by default.
- **Metrics**: Core and backend components MUST expose Prometheus-compatible
  metrics including: audio pipeline latency histograms, buffer underrun counts,
  decode times per format, library scan throughput, IPC message rates, and
  federation sync lag. Metrics MUST be available both locally (embedded HTTP
  endpoint) and via the backend's metrics aggregation.
- **Distributed tracing**: Requests that span multiple components (e.g., a
  "play" command originating from a web client, routed through the backend to
  the engine) MUST propagate trace context via OpenTelemetry. Sampling rate
  MUST be configurable, defaulting to 1% for production.
- **Performance profiling**: The release binary MUST include an optional
  profiler endpoint (feature-gated) that produces CPU flame graphs, allocation
  profiles, and lock contention reports on demand. Cold-start and steady-state
  profiles MUST be captured in every release benchmark suite.
- **Audio pipeline introspection**: The engine MUST expose real-time pipeline
  state: current latency at each pipeline stage, DSP processing time per effect,
  buffer fill levels, and format decode throughput.
- **Observability as data**: All metrics, traces, and profiles are themselves
  subject to the data privacy guarantees of Principle XII. Telemetry of
  observability data to external services is opt-in only.

**Rationale**: In a system this layered — codecs → DSP graph → routing matrix →
IPC → protocol facade → federated backend — silent failures are the enemy.
Without deep observability, a user reporting "playback stutters" could be
debugging anything from a flaky network source to a misbehaving DSP plugin to
CPU throttling. Structured observability makes the invisible visible.

### XVI. Universal Audio Content

around MUST treat all forms of audio content as first-class citizens.
Artificial distinctions between "music," "podcasts," and "audiobooks" are
user-level conventions, not architectural boundaries.

- **Unified content model**: Music, podcasts, audiobooks, internet radio
  stations, live streams, ambient soundscapes, and any future audio content
  type MUST share the same `Track` data model. Content type is a tag, not a
  separate system.
- **Type-aware behavior**: While the data model is unified, playback behavior
  MUST adapt to content type: podcasts and audiobooks MUST remember playback
  position and support variable speed (0.5x–3.0x) with pitch correction; radio
  streams MUST handle indefinite duration; music albums MUST support gapless
  transitions.
- **Content-appropriate metadata**: Different content types have different
  metadata needs (podcast: episode number, show title, publication date;
  audiobook: chapter markers, narrator; radio: station name, now-playing).
  The metadata system MUST accommodate all of these without forcing music-
  centric assumptions on non-music content.
- **Library coexistence**: A user's library MUST seamlessly contain all content
  types. A smart playlist MAY mix music and podcast episodes. The queue MUST
  accept any content type. Filtering by type is available but never mandatory.

**Rationale**: A user who listens to music, podcasts, and audiobooks does not
want three separate applications with three separate play queues, three
configuration systems, and three keyboard shortcut sets. Content is content.
around's job is to play it — the user decides what "it" is.

## Architecture & Technology Constraints

- **Language**: Rust (stable channel). The project targets the latest stable
  Rust compiler. The cloud backend shares the same Rust codebase — no
  polyglot server.
- **GUI Framework**: The desktop GUI MUST use `iced` — a pure-Rust, retained-mode
  GUI framework with no external rendering dependencies. All UI components MUST
  be `iced` widgets.
- **Audio Output & Routing**: Platform-native backends (CoreAudio on macOS,
  PulseAudio/ALSA/PipeWire on Linux, WASAPI on Windows). A cross-platform
  abstraction (e.g., `cpal`) may serve as the initial device interface. The
  audio graph MUST support a **routing matrix** — any source MAY be routed to
  any combination of output sinks (local device, virtual patchbay, network
  stream), with independent DSP chains per route.
- **Plugin System**: Compile-time trait dispatch is the primary extension
  mechanism. Dynamic loading (`libloading`) is supported for third-party binary
  extensions. A stable C ABI is required at binary extension boundaries.
- **Extension Registry**: Extension discovery MUST be decentralized. The core
  supports multiple registry sources — official registry, third-party
  registries, local filesystem paths, and Git URLs. Users freely configure
  which sources to trust. Registry entries MUST be cryptographically signed
  by their authors. No single registry is privileged or mandatory.
- **Extension Trust Model**: Extensions run in the same address space as the
  core. Security relies on user-audited trust: code signatures verify
  authorship and integrity, but no runtime sandboxing is enforced. The
  extension manager MUST display signature status and source provenance before
  loading. Users are responsible for vetting extensions they install — the
  tooling makes this information visible but does not make trust decisions on
  the user's behalf.
- **Metadata**: The player MUST extract and display metadata (title, artist,
  album, cover art) via a shared `Metadata` trait. Metadata reading is part of
  the decoder contract. Audio fingerprinting MUST use an established acoustic
  fingerprinting library.
- **Error Handling**: All fallible operations MUST return `Result` types. Panics
  in extension code MUST be caught at the extension boundary and converted to
  errors. The core MUST never crash due to a misbehaving extension.
- **Concurrency**: Audio decoding and playback MUST occur off the main thread.
  The UI thread MUST remain responsive at all times. The IPC dispatcher MUST
  use a multi-producer, single-consumer channel architecture.
- **Configuration Backend**: Configuration MUST use a layered, mergeable backend
  (e.g., `figment` or a custom merge chain). Schema MUST be defined via `serde`
  with exhaustive validation. Filesystem watching MUST use a cross-platform
  abstraction (e.g., `notify`).
- **Engine/UI Separation**: The playback engine MUST run in its own process or
  thread, exposing a structured IPC or in-process channel API that UIs consume.
  No UI code (TUI, GUI, CLI output) may live in the engine crate.
- **Internationalization**: All user-visible strings MUST be translatable via a
  standard i18n framework (e.g., Fluent). The architecture MUST support RTL
  languages. English is the fallback and default locale. Translations are
  community-contributed.
- **Accessibility**: GUI interfaces MUST be fully navigable by keyboard and
  compatible with screen readers (NVDA, VoiceOver, Orca). TUI interfaces MUST
  function correctly in accessible terminal emulators. All interactive elements
  MUST expose accessible labels.
- **Distribution**: The project MUST be distributable via: `cargo install`,
  pre-built GitHub Release binaries, Flatpak, AppImage, Homebrew, Docker, and
  system package managers (apt, rpm, pacman). The standard product name is the
  all-lowercase string `around` — never capitalized, never suffixed.
- **Cloud Backend**: The backend is a Rust binary shipped from the same
  repository. It MUST be deployable via Docker, a single static binary, and
  `cargo run`. The backend exposes the same gRPC/protobuf API that the CLI
  and GUI use internally — clients connect to a local or remote engine
  transparently.
- **Playback Transitions**: The playback engine MUST support sample-accurate
  gapless transitions (critical for classical, live albums, and DJ mixes).
  Configurable crossfade duration with independent fade curves per track.
  Beat-matched transitions (BPM-aware crossfading) are a goal for v1.
  Transition effects MUST be implementable as extensions via the `Effect` trait.
- **Lyrics Engine**: Lyrics MUST be sourced from embedded ID3/Vorbis tags,
  external `.lrc` files, and pluggable online search backends. Display MUST
  support synced word-by-word highlighting (karaoke mode) and translated
  lyrics in a secondary language.
- **Dependency Policy**: Pure Rust crates are preferred for all functionality.
  C FFI bindings are acceptable only when no production-quality pure Rust
  alternative exists in the target domain (e.g., certain audio codecs).
  Each non-Rust dependency MUST be documented with its justification.
  Transitive dependency trees MUST be audited for supply chain hygiene.
- **Supply Chain Security**: Builds MUST be byte-reproducible. Every release
  MUST ship with a signed SBOM (Software Bill of Materials). Release
  artifacts MUST be cryptographically signed. CI MUST run `cargo vet` and
  `cargo deny` to block known-vulnerable or unvetted dependencies.
  Dependencies SHOULD be vendored for offline reproducibility.
- **Audio Visualization**: The GUI MUST include a GPU shader-driven audio
  visualization engine. Real-time audio features (FFT bins, waveform samples,
  beat detection) MUST be streamed to a GPU buffer. Users MAY write custom
  GLSL/WGSL shaders for visualization. Visualization MUST have negligible
  impact on playback performance.
- **Multi-Device Sync**: Two complementary models:
  - **LAN Sync**: Local network synchronized playback across multiple around
    instances on the same subnet. Uses multicast clock synchronization (PTP
    or custom protocol) for sub-millisecond timing accuracy. No internet
    required. Primary use case: multi-room or multi-speaker setups within a
    single home.
  - **Federated Sync**: Cross-internet synchronized playback via the federated
    backend (Principle XIV). Clock synchronization over WAN with adaptive
    buffering.
  The audio routing matrix MUST support directing output to another around
  instance (local or remote) as a first-class output sink. LAN sync is a v1
  stretch goal; federated sync is deferred.
- **Licensing**: The core and built-in extensions are licensed under the MIT
  license (see `LICENSE`). Third-party extensions may use any license.

## Development Workflow & Quality Gates

- **Branching**: Feature branches are named `###-feature-name` and created via
  the speckit workflow. Direct commits to `master` are prohibited.
- **Code Review**: Every change MUST be reviewed before merge. Reviews MUST
  verify compliance with all applicable constitution principles.
- **Architecture Decisions**: Major technical decisions (trait design, protocol
  choices, cross-cutting architectural changes) MUST be documented as ADRs
  (Architecture Decision Records) in `docs/adr/`. Each ADR captures the
  context, considered alternatives, chosen approach, and consequences.
  ADRs are immutable once merged; superseded ADRs reference their replacement.
- **CI Gates** (applied to every PR and merge):
  1. `cargo fmt --check` — formatting compliance.
  2. `cargo clippy -- -D warnings` — lint compliance.
  3. `cargo test` — all tests pass.
  4. `cargo build --release` — release build succeeds on all target platforms.
  5. `cargo bench` — performance benchmarks pass (no >10% regression).
  6. `cargo vet` — all dependencies vetted.
  7. `cargo deny check` — no banned licenses, no known vulnerabilities.
- **Test Coverage**: Contract tests are mandatory for every new `Decoder`,
  `Source`, `Effect`, or `Sink` implementation. Integration tests are mandatory
  for end-to-end playback pipelines. Unit tests are mandatory for all
  non-trivial logic.
- **Documentation**: Every public trait, struct, and function MUST have
  doc-comments. Every extension crate MUST include a README describing the
  supported formats, sources, effects, or sinks. The project architecture book
  MUST use literate programming: prose interleaved with executable and
  verifiable code examples. Rust doc-tests serve as the primary literate
  programming vehicle.
- **Breaking Changes**: Trait changes to `Decoder`, `Source`, `Metadata`,
  `Effect`, or `Sink` are breaking and require a MAJOR version bump. Extension
  API additions require a MINOR bump. Internal refactors and fixes require a
  PATCH bump.
- **API Stability**: During the pre-1.0 development phase, core traits may
  evolve rapidly with no backward compatibility guarantees. Once the project
  reaches 1.0 maturity, the project transitions to a strict SemVer +
  deprecation-cycle model: breaking changes to stable traits MUST be
  announced with a `#[deprecated]` annotation at least one MINOR release before
  removal. New capabilities are added via trait extension or new traits rather
  than modifying existing stable contracts. The set of stabilized traits is
  explicitly documented per release.

## Governance

This constitution supersedes all other development practices and conventions.
Any deviation from these principles MUST be explicitly documented with a
justification in the implementation plan's Complexity Tracking table.

**Project Governance**: around operates under a BDFL (Benevolent Dictator For
Life) model. The project founder holds final decision-making authority on
architecture, feature scope, and design direction. Community contributions are
welcomed via Issues and Pull Requests, and substantive discussions occur in the
open — but the BDFL reserves the right to make the final call to maintain
architectural coherence and prevent design-by-committee.

**Amendment Procedure**:
1. Propose the amendment with a rationale document.
2. Review and approve via PR against the constitution file.
3. Update the version and `LAST_AMENDED_DATE`.
4. Propagate changes to affected templates and guidance documents.

**Versioning Policy**: `MAJOR.MINOR.PATCH` following semantic versioning:
- **MAJOR**: Backward-incompatible governance changes, principle removals, or
  trait contract changes.
- **MINOR**: New principles, new sections, materially expanded guidance.
- **PATCH**: Clarifications, wording fixes, non-semantic refinements.

**Compliance Review**: Every PR description MUST include a "Constitution
Compliance" section confirming adherence to each principle. Reviewers MUST
reject PRs that violate non-negotiable principles without documented
justification.

**Version**: 1.8.0 | **Ratified**: 2026-05-26 | **Last Amended**: 2026-05-26
