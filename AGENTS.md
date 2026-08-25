<!-- SPECKIT START -->
For additional context about technologies to be used, project structure,
shell commands, and other important information, read the current plan
<!-- SPECKIT END -->
<!-- Keep this block below the SPECKIT block. -->
## Agent skills

### Issue tracker

GitHub Issues on `vcup/around`. See `docs/agents/issue-tracker.md`.

### Triage labels

Default canonical label strings (no overrides). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context repo (transitioning to multi-context when bounded contexts
emerge). See `docs/agents/domain.md`.

## Comment preservation

Comments are part of the implementation. When editing existing code, preserve
comments unless the documented behaviour is intentionally removed or the
comment is rewritten to remain accurate. Deleting comments as incidental
formatting or refactoring collateral is prohibited. Any intentionally removed
comment MUST be replaced by updated documentation when the behaviour remains
relevant.
