# Issue tracker: GitHub

Issues and PRDs for this repo live as GitHub Issues on `vcup/around`.

## GitHub (primary)

Use the `gh` CLI for all operations.

### Conventions

- **Create an issue**: `gh issue create --title "..." --body "..."`. Use a heredoc
  or `--body-file` for multi-line bodies.
- **Read an issue**: `gh issue view <number> --comments`. Use `--json` for
  machine-readable output.
- **List issues**: `gh issue list --json` with appropriate `--label` filters.
- **Comment on an issue**: `gh issue comment <number> --body "..."`.
- **Apply / remove labels**: `gh issue edit <number> --add-label "..."` /
  `--remove-label "..."`. Multiple labels can be comma-separated.
- **Close**: `gh issue close <number> --comment "..."` (posts the comment and
  closes in one call).

## When a skill says "publish to the issue tracker"

Create a GitHub issue.

## When a skill says "fetch the relevant ticket"

Run `gh issue view <number> --comments`.
