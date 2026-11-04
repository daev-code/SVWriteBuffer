# AGENTS.md - SVWriteBuffer

A 32-bit Rust DLL for the WoW 1.12 client.

- `README.md` is for players: what it does, safety, install, the log. Keep developer
  detail out of it.
- `CONTRIBUTING.md` covers build, testing, releases and the rules for changes. Follow it.
- The module comment in `src/lib.rs` explains the mechanism.

Keep all three in sync with the code.

## Pre-commit checklist

1. Build and test as in `CONTRIBUTING.md`. Say what wasn't tested.
2. No personal details, local paths, account or character names, or the specific client,
   launcher, server or realm, in files, commit messages or release notes. Say "WoW 1.12".
3. Conventional Commits.
4. Before pushing a change players will notice, bump `version` in `Cargo.toml` once per
   push. Minor for a `feat:`, patch for fixes only, nothing for docs. Update the README's
   log example to match.

## Workflows

Pull requests must never run anything with write access. Never add `pull_request_target`
or `workflow_run` triggers, and never give a `pull_request` workflow write permissions or
secrets. Use only GitHub's own `actions/*` actions, pinned to a major version.

## Releases

Never create a release, push a `v*` tag or upload a DLL by hand. The workflow does it.

- To check before pushing: `git tag vX.Y.Z`, `tools/release-check.sh vX.Y.Z`, then
  `git tag -d vX.Y.Z`.
- On the draft release, write what changed for players, in plain words, above the
  SHA-256 and verify lines. Then publish.
- If the check fails on GitHub, nothing is tagged. Fix it on `main`. If the fix doesn't
  touch `Cargo.toml`, rerun it: Actions > Release > Run workflow, on `main`.
