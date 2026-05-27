# Requirements Quality Checklist: Core Audio Playback Engine

**Purpose**: Validate completeness, clarity, consistency, and coverage of requirements in the spec, contracts, and plan.
**Created**: 2026-05-27
**Evaluated**: 2026-05-27
**Resolved**: 2026-05-27
**Feature**: [spec.md](../spec.md) | [plan.md](../plan.md) | [contracts/](../contracts/)

**Note**: This checklist validates requirements quality per the `/speckit.checklist` framework — testing the requirements themselves, not the implementation.

---

## Requirement Completeness

- [x] CHK001 — Are explicit functional requirements defined for every edge case listed in the spec? [Completeness, Spec §Edge Cases]
  → **Resolved**: Covered by FR-010 (VLC skip), FR-006 (device disconnect), FR-017 (DRM/CodecNotSupported), FR-006 (concurrent FIFO), and decoder contract (seek beyond duration). Zero-duration accepted via DecodeError.

- [x] CHK002 — Are requirements defined for the IPC socket lifecycle? [Gap, Spec §FR-004/FR-006]
  → **Resolved**: FR-006 now specifies 0600 permissions, connect-check-before-bind on startup, stale socket deletion.

- [x] CHK003 — Are requirements specified for stale socket from previous crashed instance? [Gap, Spec §FR-006]
  → **Resolved**: FR-006: connect to existing socket; if connection fails, delete stale socket and recreate.

- [x] CHK004 — Are error response formats specified for ALL IPC commands? [Completeness, Contract §IPC Commands]
  → **Open (minor)**: pause, resume, stop, status, list_decoders lack documented error responses. Low priority — few failure modes for these commands.

- [x] CHK005 — Are volume control requirements defined? [Gap, Contract §IPC Status]
  → **Resolved**: Removed `"volume"` field from IPC status response. Volume control is OS-managed in MVP.

- [x] CHK006 — Are requirements defined for `load_decoder` with invalid shared library path? [Completeness, Spec §US3]
  → **Pass**: IPC contract covers `DECODER_LOAD_FAILED` error. No change needed.

- [x] CHK007 — Are extension unload requirements defined for in-progress playback? [Completeness, Spec §US3 Scenario 3]
  → **Open (minor)**: Covered in acceptance scenario. FR restatement optional.

- [x] CHK008 — Are format detection requirements defined for both-extension-and-magic-bytes-fail? [Gap, Spec §FR-002]
  → **Open (minor)**: `UnsupportedFormat` error covers this implicitly. Acceptable for MVP.

- [x] CHK009 — Are requirements defined for `load_decoder_bytes` temp file cleanup on SIGKILL? [Gap, Spec §FR-016b]
  → **Resolved**: FR-016 adds `cleanup` command to remove stale temp files. SIGKILL not handled automatically (extreme case).

## Requirement Clarity

- [x] CHK010 — Is "gracefully" in FR-010 quantified? [Clarity, Spec §FR-010]
  → **Pass**: Defined as "returning structured errors rather than panicking." Decoder contract reinforces. No change needed.

- [x] CHK011 — Is "immediately" (Ctrl+C) quantified? [Clarity, Spec §US1 Scenario 2 vs SC-008]
  → **Open (minor)**: Bound not explicitly stated for Ctrl+C path. Naturally shared with SC-008 (500ms).

- [x] CHK012 — Is "within 2 seconds" (SC-001) measurement start point clear? [Ambiguity, Spec §SC-001]
  → **Pass**: "Within 2 seconds of invoking the command." Clear. No change needed.

- [x] CHK013 — Is "consumer hardware" defined with baseline? [Ambiguity, Spec §SC-001/SC-004]
  → **Resolved**: CI runner used as reference measurement platform. Documented in SC and assumptions.

- [x] CHK014 — Are system-wide search paths enumerated per platform? [Clarity, Spec §FR-007]
  → **Resolved**: FR-007 now specifies `/usr/lib/around/decoders/` (Linux), `/Library/Application Support/around/decoders/` (macOS), `%PROGRAMDATA%\around\decoders\` (Windows).

- [x] CHK015 — Is "position" unit defined? [Ambiguity, Spec §FR-005 vs Contract]
  → **Pass**: Data model defines milliseconds. IPC contract uses `position_ms`. No change needed.

- [x] CHK016 — Is `Decoder::seek()` offset unit specified? [Ambiguity, Contract §Decoder Trait]
  → **Resolved**: Sample frame offset. Engine converts ms → sample frames. Decoder contract updated.

- [x] CHK017 — Is "concurrent commands" scoped? [Ambiguity, Spec §Edge Cases]
  → **Resolved**: Addressed by FR-006 (FIFO serialization of all IPC commands).

- [x] CHK018 — Are "corrupted" and "truncated" defined with distinguishing criteria? [Ambiguity, Spec §Edge Cases]
  → **Open (minor)**: Acceptable for MVP. Test fixtures distinguish them (truncated.wav vs corrupt file).

## Requirement Consistency

- [x] CHK019 — Does PlaybackStatus align with IPC `"volume"` field? [Consistency]
  → **Resolved**: Volume field removed from IPC status. OS-managed volume in MVP.

- [x] CHK020 — Do SampleSpec channel validation and ChannelLayout constants align? [Consistency]
  → **Pass**: Constants cover consumer range (1-8). Validation allows professional range (9-32). Documented.

- [x] CHK021 — Does "dummy Source" test fixture conflict with FR-009? [Consistency]
  → **Pass**: Test-only artifact. No conflict with production Source requirement.

- [x] CHK022 — Are IPC error codes and AroundError variants one-to-one? [Consistency]
  → **Resolved**: Added `AroundError::InvalidPosition` variant. All IPC error codes now have corresponding variants.

- [x] CHK023 — Does KDL unknown-key rejection align with extension config? [Conflict, Spec §FR-014 vs §FR-007]
  → **Resolved**: Switched to `kdl-rs`. Core config strictly validated; `[extensions]` node children passed through without validation.

## Acceptance Criteria Quality

- [x] CHK024 — Can SC-003 (<50MB RSS) be objectively measured? [Measurability, Spec §SC-003]
  → **Resolved**: Linux `/proc/self/statm` resident field, 10 samples at 1s intervals, 30s idle, median value. Documented in SC-003.

- [x] CHK025 — Can SC-004 (latency <10ms) be objectively measured? [Measurability, Spec §SC-004]
  → **Resolved**: Hard 10ms target removed. Internal pipeline latency measured; best-effort minimization. End-to-end via loopback where available.

- [x] CHK026 — Can SC-005 (≤6 methods) be verified given 8 methods? [Measurability, Spec §SC-005]
  → **Resolved**: SC-005 updated to "no more than 8 required methods" (4 Decoder + 4 DecoderFactory).

- [x] CHK027 — Does SC-007 (cold boot <500ms) include extension discovery? [Measurability, Spec §SC-007]
  → **Resolved**: Excludes extension discovery. Extension scanning is lazy-loaded on first `list_decoders` or `play`.

- [x] CHK028 — Does SC-008 (exit <500ms) include decoder teardown? [Measurability, Spec §SC-008]
  → **Resolved**: Engine teardown only. Decoder teardown is asynchronous with independent 2-second timeout.

## Scenario Coverage

- [x] CHK029 — Are requirements defined for `play` while another track already playing? [Coverage, Spec §US2]
  → **Resolved**: New `play` stops current track and starts new source immediately (VLC-style replace). Documented in clarifications and FR-004.

- [x] CHK030 — Are requirements defined for IPC client disconnecting mid-command? [Coverage, Spec §FR-006]
  → **Pass**: Newline-delimited JSON naturally handles partial messages (no newline = discard). No additional requirement needed.

- [x] CHK031 — Are requirements defined for KDL malformed vs unknown keys? [Coverage, Spec §FR-014]
  → **Resolved**: FR-014 with `kdl-rs` distinguishes parse errors (malformed KDL) from schema errors (unknown keys). Both produce distinct messages.

- [x] CHK032 — Are requirements defined for decoder fallback on `open()` failure? [Coverage, Spec §FR-002]
  → **Resolved**: FR-002 now specifies fallback priority: (1) same-format decoders in registration order, (2) all other decoders.

- [x] CHK033 — Are state transition rules defined for PlaybackStatus? [Gap, Spec §US2]
  → **Resolved**: States: Playing, Paused, Stopped, Buffering. Seek is a command (not a state). State machine defined in FR-005.

- [x] CHK034 — Are requirements defined for `load_decoder_bytes` disk-full? [Coverage, Spec §FR-016b]
  → **Open (minor)**: Temp file write errors handled gracefully by tempfile crate. Explicit requirement optional.

## Edge Case Coverage

- [x] CHK035 — Is "truncated/corrupt mid-playback" mapped to explicit FR? [Edge Case, Spec §Edge Cases]
  → **Resolved**: FR-010: VLC-style — skip damaged frames, continue. Stop after 3 consecutive errors (configurable). Error variant and logging per FR-015.

- [x] CHK036 — Is "format detected but decode fails (DRM)" mapped to explicit FR? [Edge Case, Spec §Edge Cases]
  → **Resolved**: FR-017: `CodecNotSupported` error with codec name and reason. Distinguishes from `UnsupportedFormat`.

- [x] CHK037 — Is "audio output device disconnected" mapped to explicit FR? [Edge Case, Spec §Edge Cases]
  → **Resolved**: FR-006: Auto-pause on device loss, report via IPC status. Recovery configurable per device type.

- [x] CHK038 — Is "zero duration / no decodable samples" mapped to explicit FR? [Edge Case, Spec §Edge Cases]
  → **Open (minor)**: Zero-byte files fail at decoder open (DecodeError). Valid headers with zero data handled by decoder. Current behavior acceptable.

- [x] CHK039 — Is "seek beyond file duration" mapped to explicit FR? [Edge Case, Spec §Edge Cases]
  → **Pass**: IPC contract defines `INVALID_POSITION` error. FR-017 adds `InvalidPosition` error variant.

- [x] CHK040 — Is "concurrent commands" mapped to explicit FR? [Edge Case, Spec §Edge Cases]
  → **Resolved**: FR-006: All IPC commands serialized, processed in FIFO order. Last-arriving command reflects final state.

- [x] CHK041 — Is "duplicate `load_decoder`" defined? [Edge Case, Contract §IPC Load Decoder]
  → **Resolved**: FR-016: Same path + version → idempotent (return existing). Same path + different version → replace.

## Extension API Stability

- [x] CHK042 — Is "stable Decoder trait" accompanied by concrete criteria? [Ambiguity, Spec §FR-008 vs Contract]
  → **Pass**: Contract marks "Pre-1.0 (may evolve)." Spec qualification resolved by contract.

- [x] CHK043 — Is a versioning policy defined for Decoder trait? [Gap, Contract §Decoder Trait]
  → **Deferred**: Pre-1.0 versions carry no compatibility contract. Formal versioning policy defined at v1.0.0.

- [x] CHK044 — Are backward compatibility requirements defined for trait evolution? [Gap, Spec §FR-008]
  → **Resolved**: v0.x: no guarantees, recompile per version. v1.0+: semver. Documented in decoder contract and FR-008.

- [x] CHK045 — Are ABI stability requirements defined for `create_decoder` FFI? [Gap, Spec §FR-003a, Research]
  → **Deferred**: ABI stability guarantees deferred to v1.0.0. Current `Box<dyn Decoder>` accepted for pre-1.0. Noted in FR-003a.

- [x] CHK046 — Is Source trait held to same stability criteria as Decoder? [Consistency, Spec §FR-008 vs §FR-009]
  → **Deferred**: Source contract stability updated to parity with Decoder. Formal policy at v1.0.0.

## Non-Functional Requirements

- [x] CHK047 — Are IPC socket security requirements defined? [Gap, NFR]
  → **Deferred**: Post-MVP. Socket permissions 0600 (FR-006) provides basic protection.

- [x] CHK048 — Are CLI accessibility requirements defined? [Gap, NFR]
  → **Deferred**: Post-MVP. CLI text output is inherently screen-reader compatible.

- [x] CHK049 — Are internationalization requirements defined? [Gap, NFR]
  → **Deferred**: Post-MVP. English-only CLI acceptable for MVP.

- [x] CHK050 — Are signal handling requirements defined beyond Ctrl+C? [Gap, Spec §US1 Scenario 2]
  → **Resolved**: FR-004: SIGINT, SIGHUP, SIGTERM share cleanup sequence (stop playback, close socket, release audio, delete socket, exit).

## Dependencies & Assumptions

- [x] CHK051 — Is "identical performance across load modes" validated by measurement methodology? [Assumption, Spec §Assumptions]
  → **Pass**: Research defines benchmark methodology (<1% throughput difference). No change needed.

- [x] CHK052 — Does `cpal` meet latency requirements? [Assumption, Research §Audio Output]
  → **Resolved**: Hard 10ms target removed (SC-004). Best-effort minimization across platforms. Platform audio subsystem differences acknowledged.

- [x] CHK053 — Is `tokio` current_thread "<2MB RSS" validated? [Assumption, Research §Async Runtime]
  → **Open (minor)**: Assertion without measurement. 2MB figure is plausible but unverified.

- [x] CHK054 — Are external dependency risks documented? [Gap, Plan §Primary Dependencies]
  → **Open (minor)**: Research documents decisions and alternatives. Maintenance risks (bus factor, known issues) not catalogued.

## Ambiguities & Conflicts

- [x] CHK055 — Does "foreground" imply terminal process group or just lifetime? [Ambiguity, Spec §FR-004]
  → **Open (minor)**: Standard Unix behavior — foreground = terminal-attached (Ctrl+C works), backgroundable with `&`. Acceptable.

- [x] CHK056 — Does `Source::open() -> Box<dyn Read>` conflict with "no dyn Trait in hot paths"? [Conflict]
  → **Pass**: `open()` is called once at pipeline start. I/O dominates dispatch overhead. No conflict.

- [x] CHK057 — Is decoder contract §4 "either behavior is valid" intentional? [Ambiguity, Contract §Decoder Trait]
  → **Open (minor)**: Permissive ambiguity. Source trait's MULTI_OPEN flag already signals reopen support. Acceptable for MVP.

- [x] CHK058 — Is IPC `"state"` vs `"status"` naming intentional? [Ambiguity, Contract §IPC Commands]
  → **Pass**: "status" = envelope (ok/error); "state" = playback state value. Distinct semantics. Consistent usage.

---

## Summary

| Category | Resolved | Pass | Deferred | Open | Total |
|---|---|---|---|---|---|
| Requirement Completeness | 4 | 1 | 0 | 4 | 9 |
| Requirement Clarity | 3 | 3 | 0 | 3 | 9 |
| Requirement Consistency | 3 | 2 | 0 | 0 | 5 |
| Acceptance Criteria Quality | 5 | 0 | 0 | 0 | 5 |
| Scenario Coverage | 5 | 0 | 0 | 1 | 6 |
| Edge Case Coverage | 5 | 1 | 0 | 1 | 7 |
| Extension API Stability | 1 | 1 | 3 | 0 | 5 |
| Non-Functional Requirements | 1 | 0 | 3 | 0 | 4 |
| Dependencies & Assumptions | 0 | 1 | 0 | 3 | 4 |
| Ambiguities & Conflicts | 0 | 3 | 0 | 1 | 4 |
| **Total** | **27** | **12** | **6** | **13** | **58** |

**Resolved**: 27 items addressed via spec/contract/code changes.
**Pass (no action)**: 12 items already adequate.
**Deferred**: 6 items deferred to post-MVP or v1.0.0 (ABI stability, versioning policy, NFRs).
**Open (minor)**: 13 items with low-impact residual gaps — acceptable for MVP.

### Files Updated

| File | Changes |
|---|---|
| `spec.md` | FR-002, FR-004, FR-005, FR-006, FR-007, FR-010, FR-013, FR-014, FR-016, FR-017 (new), SC-003, SC-004, SC-005, SC-007, SC-008, assumptions |
| `data-model.md` | PlaybackStatus: added BUFFERING=3 |
| `contracts/decoder-trait.md` | seek() unit, trait split note, backward compatibility policy |
| `contracts/ipc-commands.md` | Removed volume field, added cleanup command, buffering state |
| `contracts/source-trait.md` | Stability parity with Decoder |
| `research.md` | KDL: knuffel → kdl-rs |
| `plan.md` | KDL dependency: knuffel → kdl-rs |
| `crates/around-core/src/error.rs` | Added InvalidPosition, CodecNotSupported variants |
| `crates/around-core/src/state.rs` | Added BUFFERING=3 |
| `crates/around-core/src/decoder.rs` | seek() doc: sample frame offset |
| `crates/around-engine/src/pipeline.rs` | VLC-style error recovery (skip+continue, stop after 3 consecutive) |
