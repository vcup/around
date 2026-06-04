# ADR 0001: Documentation Language Convention

English is the default authoring language for all project documentation
(CONTEXT.md, ADRs, READMEs, specs, doc comments). Agent thinking and code
output default to English. Agent conversations with the user may be in
Chinese.

## Status

Accepted

## Context

The around project is built by a Chinese-speaking developer, but:

- The codebase (identifiers, comments, commit messages) is in English per
  Rust ecosystem convention.
- LLM agents operate most reliably when their thinking and output are in
  English — reasoning quality and terminology consistency degrade when
  agents produce documentation in a non-English language.
- Open-source contributors who do not read Chinese should be able to
  understand the project's architecture and domain model.
- The design-docs reference (CliniNg project) established this pattern
  with a Chinese mirror tree at `docs/zh-Hans/`.

## Decision

1. All documentation is written in English as the authoritative version.
2. Agent conversations with the user may be in Chinese — this is
   user-facing communication, not documentation.
3. Agent thinking, code comments, and documentation output default to
   English.
4. A Chinese mirror at `docs/zh-Hans/` will be introduced when the
   project accumulates enough documentation to justify the maintenance
   cost.

## Consequences

### Positive
- Agents operate in English (LLM-native) → higher reasoning quality,
  fewer hallucinations, consistent terminology.
- Non-Chinese contributors have access to the single English source of
  truth.
- Aligns with the CliniNg reference pattern, which already validated
  this approach in practice.

### Negative
- The mirror tree adds maintenance overhead (deferred until it becomes
  cost-justified).
