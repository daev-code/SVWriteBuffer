# Contributing

Issues and pull requests are welcome.

## Commits

Use [Conventional Commits](https://www.conventionalcommits.org/): `feat`, `fix`, `docs`,
`refactor` or `chore`, then a short summary. PRs are squash-merged and the PR title
becomes the commit message, so title PRs the same way. For example:
`fix: flush the buffer before GetFileSizeEx`.

## Build

The client is 32-bit, so you need Rust with the `i686-pc-windows-msvc` target:

```
rustup target add i686-pc-windows-msvc
cargo build --release
```

The DLL ends up in `target\i686-pc-windows-msvc\release\SVWriteBuffer.dll`. The build
must have no warnings.

The version lives only in `Cargo.toml`. `build.rs` copies it into the DLL's Properties,
and the log's first line shows it.

## Test

```
cargo run --release --example harness -- <full path to SVWriteBuffer.dll>
```

The harness writes a test file token by token, like the client, with and without the DLL.
It checks that:

- the contents match,
- a size check mid-write sees everything written so far,
- other files aren't buffered,
- calls through function addresses saved before the DLL loaded are buffered too,
- the log shows file names only, with no paths or folder names.

It must print `ALL OK`.

If you change the hooks, also test in game. Restart the client, `/reload`, and read the
log. Check that addon settings survive a reload and a relog. Say in the PR what you
tested.

## Releases

GitHub Actions builds every release, only from `main`. Bumping `version` in `Cargo.toml`
on `main` starts one. The workflow runs `tools/release-check.sh`, which refuses a
mismatched version, among other checks. Then it tags the commit `vX.Y.Z` and creates a
draft release with the DLL.

Each release DLL gets a GitHub build provenance attestation, and its SHA-256 goes in the
release notes. Anyone can check a download with
`gh attestation verify SVWriteBuffer.dll --repo daev-code/SVWriteBuffer`.

## How it works

The DLL redirects kernel32's own import entries for the file functions, in memory. It
buffers writes to SavedVariables files and writes each file in one call when it's
closed. The module comment at the top of `src/lib.rs` explains it in full.

## Rules for changes

- Data must be on disk before `CloseHandle` returns. No background writing.
- Anything that can observe a tracked file (read, seek, size, truncate, flush) writes the
  buffer out first.
- Only patch kernel32's import entries, in memory. Never patch code, game memory or Lua.
- Never bypass another DLL's hook. Patch nothing unless open, write and close can all be
  covered.
- Untracked handles go straight to kernelbase.
- Never log paths or folder names. Players attach the log to public issues, and the
  folders hold account, realm and character names.
- The DLL's own logging must run inside the `BUSY` flag, so the hooks pass it through.
  Otherwise it recurses or deadlocks.
- Only write a tracked handle through the hooked `WriteFile`. Anything else, like
  `NtWriteFile` (used by Rust's `std::fs`), would land before the buffered data. That's
  why the harness calls `CreateFileW` and `WriteFile` directly.
- Every hook body runs inside `catch_unwind`. Don't set `panic = "abort"`.
- Dynamic CRT only. Don't add `+crt-static`: the game already has a CRT loaded.
- The DLL is always named `SVWriteBuffer.dll`, with no version, so players never have to
  edit `dlls.txt` again.
