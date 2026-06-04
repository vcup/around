# Domain Docs

How the engineering skills should consume this repo's domain documentation when
exploring the codebase.

## Layout

Single-context repo. One `CONTEXT.md` at the repo root serves as the unified
glossary. Terms are organised into sections that correspond to future bounded
contexts (`around-core`, `around-engine`). When a section accumulates enough
terms to stand alone (generally >5, or when an agent must cross-reference
unrelated sections to understand a single concept), it will split into its own
per-context `CONTEXT.md` under `crates/<context>/CONTEXT.md`. A `CONTEXT-MAP.md`
at the root will be introduced at that point.

## Before exploring, read these

- **`.specify/memory/constitution.md`** — the project's founding document.
  Defines Core Principles (immutable), Architecture Directives (strong
  commitments), and Technical Constraints. Every design decision MUST be
  consistent with it. Read this first — it is the highest authority in
  the repo.
- **`CONTEXT.md`** at the repo root — the unified glossary for all crates.
- **`docs/adr/`** — read ADRs that touch the area you're about to work in.
- **`specs/<NNN>-<name>/spec.md`** — the canonical feature specification.
  Design decisions are recorded here via the speckit workflow. When code and
  spec disagree, flag it — the spec is the authority unless an ADR overrides
  it. Note: constitution.md outranks both specs and ADRs.

If any of these files don't exist, **proceed silently**. Don't flag their
absence; don't suggest creating them upfront. The producer skill
(`/grill-with-docs`) creates them lazily when terms or decisions actually
get resolved.

## Use the glossary's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal,
a hypothesis, a test name), use the term as defined in `CONTEXT.md`. Don't drift
to synonyms the glossary explicitly avoids.

If the concept you need isn't in the glossary yet, that's a signal — either
you're inventing language the project doesn't use (reconsider) or there's a real
gap (note it for `/grill-with-docs`).

## Decision persistence

When a design decision is made that a future maintainer or agent could
reasonably reverse (because the rationale is not obvious from code), persist it
immediately to the durable file it belongs to:

- **Terminology** → `CONTEXT.md`
- **Architectural trade-off** → `docs/adr/<NNNN>-<slug>.md`
- **Agent behavioral guidance** → `docs/agents/domain.md`
- **Non-obvious code behaviour** → a comment at the exact call site or
  declaration. Comments MUST be kept current when the code changes — an
  outdated comment is worse than no comment. When refactoring, treat
  comments as part of the code: update them alongside the logic they
  describe.

Do not assume the next session inherits the current session's context.

## Known deviations (branch: `004-platform-native-ipc`)

These are temporary inconsistencies. The Codec naming deviation was fixed
in-branch (it overlapped with IPC work). The remaining deviation will be
resolved when the branch merges.

| Deviation | Code location | Resolution after merge |
|-----------|--------------|----------------------|
| `PlaybackStatus` is `u8` constants (`PLAYING`, `PAUSED`, `STOPPED`, `BUFFERING`) in `around-core/src/state.rs`, but the canonical term is the enum form | `around-core/src/state.rs`, `around-engine/src/ipc/types.rs` (`TrackState`) | Replace `u8` constants with `PlaybackStatus` enum; unify `TrackState` into it; add `Buffering` variant |
