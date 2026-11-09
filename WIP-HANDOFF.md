# WIP Handoff — Native IPC hardening

**Checkpoint:** 2026-11-09 14:06 +08:00
**Reason:** User requested an immediate pause, durable progress capture, and a WIP commit.
**Commit intent:** capture the entire current repository worktree; review the diff before any later split or merge.

## Pause state

- The background-job registry lists no active local cargo/test job. The last full verification call was interrupted before producing a result; it is not a successful verification.
- WindowsEnv and WindowsScenarios are paused and reported no lab-owned process, SSH session, build, watcher, or running container.
- Lab container `aroundipc-pBrcml-windows` exited cleanly before pause (`ExitCode=0`, `OOMKilled=false`); no Docker prune or artifact deletion occurred.
- The Windows agents made no repository edits and no commit.
- Commit signing required a GPG passphrase unavailable to this session; the
  interactive attempt timed out. The user authorized this WIP checkpoint to
  be unsigned. Use a one-command override only; leave Git/GPG configuration
  unchanged and do not assume later commits may be unsigned.

## Repository checkpoint scope

The WIP commit includes all changes currently present in the worktree, including the existing tracked edits and the untracked `crates/around-cli/tests/ipc_native.rs`. The change set covers:

- Native Unix IPC candidate discovery, stale-socket handling, permission setup, bind-error classification, cleanup of the actual bound path, and CLI native-first discovery with TCP fallback.
- Unix startup race hardening: a persistent `<socket>.lock` inode is acquired with nonblocking exclusive `flock` before probing/binding and held through listener cleanup; `BoundUnixSocket` checks device/inode before removing its endpoint.
- CLI and Unix IPC integration coverage, including ordinary-file/symlink preservation, bind fallback, server error propagation, stop-versus-shutdown lifecycle, cleanup, and concurrent stale-socket takeover.
- Documentation/spec updates describing the current IPC lifecycle, implementation deviations, and the limits of historical audio/signal acceptance criteria.
- The Unix-only `libc` dependency and resulting `Cargo.lock` update.
- Existing repository guidance/tooling changes in `AGENTS.md`,
  `.kilocode/rules/specify-rules.md`, `.kilocode/workflows/speckit.plan.md`,
  `.opencode/commands/speckit.plan.md`, and `.pi/prompts/speckit.plan.md`
  are preserved in this all-worktree checkpoint, not reverted or recategorized.

Implementation entry points: `crates/around-cli/src/main.rs`,
`crates/around-engine/src/ipc/transport_unix.rs`, and
`crates/around-engine/src/ipc/transport_manager.rs`.
Regression files: `crates/around-cli/tests/ipc_native.rs` and
`crates/around-engine/tests/ipc_unix_socket.rs`.
Updated feature documents are under `specs/001-core-playback-engine/contracts/`,
`specs/002-mvp-audio-output/`, and `specs/004-platform-native-ipc/data-model.md`.
The feature selection is `.specify/feature.json`; historical task checkmarks
are not substitutes for the verification evidence below.

## Verification evidence already obtained

### Passed

- Markdown fence balance check for the edited IPC/spec documents.
- `cargo build -p around-extensions-test-plugin` (builds the dynamic test fixture).
- `cargo fmt --all` completed after the lock fix and test-path shortening.
- Targeted regression after the lock fix:
  `cargo test -p around-cli --test ipc_native concurrent_stale_socket_takeover_has_one_owner -- --exact`
  — passed.
- Real process smoke after the lock fix: 48 simultaneous startup rounds, 3 `serve` processes per round, exactly one winner per round; 24 crash/stale-socket recoveries and 24 graceful shutdown cleanups; no TCP fallback — passed.
- The original real-process smoke reproduced the defect before the fix:
  5 of 12 rounds left two live instances. The permanent concurrent regression
  also failed before the fix with `multiple engines claimed the same socket`.
- The initial `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` reached the test phase; format/clippy completed, but the workspace test run failed because the dynamic fixture `.so` had not yet been built. Eleven `around-extensions` tests reported the missing fixture.

### Not complete

- After fixture preparation and the final Unix changes, a full verification
  command was issued but interrupted before any result was recorded. Whether
  it ran to completion is unverified; no final post-fix full clippy/workspace
  test success can be claimed.
- No native Windows build or Windows IPC execution exists. The PowerShell scenario harness was AST-parsed and self-tested only with a Linux fake; that is not Windows evidence.
- No claims are made here about audio timing, arbitrary signal handling, or Windows named-pipe behavior.

## Windows lab state and artifacts

Lab root (outside the repository):

`/mnt/disk/around-ipc-windows-pBrcml`

Durable external status:

- `/mnt/disk/around-ipc-windows-pBrcml/STATUS-PAUSED.md`
- `/mnt/disk/around-ipc-windows-pBrcml/shared/evidence/STATUS-RESUME.md`

Scenario deliverable:

- `/mnt/disk/around-ipc-windows-pBrcml/shared/scripts/run.ps1`

Preserved provisioning artifacts include the loopback-only Compose file, Windows 10 LTSC evaluation ISO, qcow2 disk, source snapshot, staged MSVC/Rust/protoc installers, and guest helper scripts. The source snapshot is:

`shared/source/around-src.tar.gz` — SHA-256 `e284ae4a2c958ce6dba617f65b8a6b2bc992baebee6538b3b06f08c35670b055`

Windows blockers at pause:

- Guest SSH was never verified; the toolchain was not installed; no `around.exe` was built; no READY signal was sent.
- Host root storage was critically full: approximately 73–77 MB free. Do not run Docker prune or delete unrelated data without explicit approval.
- Windows installation completion on the existing qcow2 disk is unconfirmed, although the container exited normally after guest ACPI shutdown.

The fake harness output under `tools/selftest-evidence/` is explicitly not Windows proof and must not be cited as such.

## Safe resume sequence

1. Read this file and the two external Windows status files; inspect `git status` and the WIP diff.
2. If full Linux verification is desired, first run `cargo build -p around-extensions-test-plugin`, then run the final format, clippy, and workspace test commands. Treat any result as new evidence; do not infer it from this checkpoint.
3. For Windows, free host root space without pruning unrelated Docker data, then boot the existing lab with `docker compose up -d` from the lab root, verify SSH with `guest-exec.sh`, run `shared/provision/setup-toolchain.ps1`, and run `build.ps1` once.
4. Only after `shared/bin/around.exe` exists and WindowsEnv sends READY, run `shared/scripts/run.ps1`; preserve its real evidence separately from the fake self-test.
5. The optional TCP fallback check is the isolated `ipc_control` configuration described in `STATUS-RESUME.md`; do not use or claim `AROUND_FORCE_TCP` because it is not implemented in this source.
6. Before any final/non-WIP commit, review the full diff and decide whether the repository-wide pre-existing edits belong in the same change.

## Suggested skills on resume

- `diagnose` for any failed race or lifecycle reproduction.
- `tdd` for additional consumer-visible IPC regression coverage.
- `handoff` when transferring this paused state to another session.
- `edit-commit-msg` only if the WIP message needs rewriting; it does not create commits.
