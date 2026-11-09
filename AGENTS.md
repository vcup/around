# Agent guidance

## Project context

Read the current feature's `plan.md` for technologies, project structure,
commands, and implementation context. Feature docs live under `specs/`;
`.specify/feature.json` records the selected feature directory.

## Agent skills

### Issue tracker

Issues and PRDs: GitHub Issues on `vcup/around`.
Follow `docs/agents/issue-tracker.md`.

### Triage labels

Use the canonical labels without overrides.
See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: root `CONTEXT.md` and `docs/adr/`; split into multiple contexts
when bounded contexts emerge. Follow `docs/agents/domain.md` for reading order,
document precedence, terminology, and decision persistence.

## Comment preservation

Treat comments as implementation. Preserve them unless the documented behavior
is intentionally removed or the comment is rewritten for accuracy. Never delete
comments as formatting or refactoring collateral. If a comment is intentionally
removed but its behavior remains relevant, you MUST replace it with updated
documentation.
