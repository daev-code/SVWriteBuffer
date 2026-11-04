#!/usr/bin/env bash
# Checks a release tag against the committed version, then builds and tests the DLL.
# Run by the release workflow. To run it locally (Git Bash), see AGENTS.md.
set -euo pipefail

tag="${1:?usage: tools/release-check.sh vX.Y.Z}"
dll="target/i686-pc-windows-msvc/release/SVWriteBuffer.dll"
fail() { echo "release-check: $*" >&2; exit 1; }

[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "tag '$tag' is not vX.Y.Z"
version=$(cargo metadata --no-deps --format-version 1 --locked \
    | grep -o '"version":"[^"]*"' | head -1 | cut -d'"' -f4)
[[ "$tag" == "v$version" ]] || fail "tag $tag does not match Cargo.toml version $version"

# The tag must point at a commit that is on main, with nothing uncommitted.
git rev-parse -q --verify "refs/tags/$tag" >/dev/null || fail "tag $tag does not exist"
tagged=$(git rev-parse "$tag^{commit}")
[[ "$tagged" == "$(git rev-parse HEAD)" ]] || fail "HEAD is not the commit tagged $tag"
[[ -z "$(git status --porcelain)" ]] || fail "working tree has uncommitted changes"
git merge-base --is-ancestor "$tagged" origin/main || fail "$tag is not on origin/main"

# Build exactly what is committed, with no warnings, and run the harness.
RUSTFLAGS="-D warnings" cargo build --release --locked
out=$(RUSTFLAGS="-D warnings" cargo run --release --locked --example harness -- \
    "$(cygpath -w "$PWD/$dll" 2>/dev/null || echo "$PWD/$dll")")
echo "$out" | tail -1
[[ "$(echo "$out" | tail -1)" == "ALL OK" ]] || fail "harness did not print ALL OK"

# The DLL's version resource must carry the same version.
file_version=$(powershell -NoProfile -Command \
    "(Get-Item '$dll').VersionInfo.FileVersion" | tr -d '\r')
[[ "$file_version" == "$version" ]] || fail "DLL file version '$file_version' is not $version"

# No absolute paths in the DLL (they would leak the build machine's folders).
if grep -a -q -E '[A-Za-z]:\\[A-Za-z0-9]' "$dll"; then
    fail "DLL contains an absolute path"
fi

echo "release-check: $tag OK"
