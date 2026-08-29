#!/usr/bin/env bash
# Rebuild the node's embedded web UI bundle (ADR-0078).
#
# opys-server/ui/dist is a *committed* build product: build.rs embeds it with
# include_bytes!, because `cargo install opys`, docs.rs and the nix sandbox all
# build with no Node and no network. So the bundle can only be regenerated
# deliberately — by this script — and CI has to prove the committed bytes still
# match their sources.
#
# Usage:
#   ui-build            # rebuild opys-server/ui/dist from opys-server/ui/src
#   ui-build --check    # rebuild and fail if the committed bundle drifted (CI gate)
#
# Node lives in the devShell only (flake.nix's devPackages); nothing in the
# crate build, and nothing in CI's cargo jobs, ever runs it.
set -euo pipefail

# Resolve the repo root from this script's location so it works from any cwd.
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$script_dir/.." && pwd)
ui="$root/opys-server/ui"
dist="$ui/dist"

check_only=false
if [[ "${1:-}" == "--check" ]]; then
  check_only=true
elif [[ $# -gt 0 ]]; then
  echo "usage: ui-build [--check]" >&2
  exit 2
fi

if [[ ! -f "$ui/package-lock.json" ]]; then
  echo "missing $ui/package-lock.json — the lockfile is committed and required" >&2
  exit 2
fi

cd "$ui"

# `npm ci`, never `npm install`: ci installs exactly the committed lockfile and
# fails if package.json and the lockfile disagree, which is what makes the drift
# gate below meaningful. --ignore-scripts because nothing in this tree needs a
# lifecycle script (the native rolldown/lightningcss binaries arrive as ordinary
# optional-dependency tarballs), so refusing to run them is free hygiene.
if ! npm ci --no-audit --no-fund --ignore-scripts; then
  echo >&2
  echo "npm ci failed. It needs the registry: the bundle is committed precisely" >&2
  echo "so only UI contributors have to be online. If you are not changing the" >&2
  echo "UI, you do not need to run this." >&2
  exit 2
fi

# Where the bundle that was in the tree before this rebuild is snapshotted.
previous=$(mktemp -d)

# Whether the EXIT trap puts that snapshot back. True until this run has earned
# the right to keep what it built, which is:
#
#   - plain mode: the build and the audit both succeeded. The new bundle is the
#     point of the run, so `restore` is cleared just before the successful exit.
#   - --check: never. A check that edits the thing it is checking is a trap —
#     the first run reports drift, the rebuild it just did becomes the new state
#     of the tree, and the *second* run passes without anything having been
#     fixed. Restoring keeps the verdict reproducible, and keeps `--check` safe
#     to run on a dirty tree.
#
# The failure case is why this is armed in both modes. `dist` is *committed* and
# `build.rs` embeds it, so a wipe followed by a failed `npm run build` (one typo
# in a Svelte file) would leave the whole cargo workspace unbuildable — not a
# UI-local failure but a red `cargo test`, `cargo clippy` and `msrv` for
# everyone in that checkout. A failed rebuild must cost nothing but the rebuild.
restore=true

# Runs from the EXIT trap so it covers every way out: a failed `npm run build`,
# an audit that bails, an interrupt. It does not disturb the exit status —
# nothing here calls `exit`.
#
# The one case that keeps a fresh build is an empty snapshot: there was no
# bundle to preserve, so putting "nothing" back would leave the tree with no
# dist/ and break the next `cargo build` just as thoroughly.
cleanup() {
  if $restore && [[ -n "$(ls -A "$previous" 2>/dev/null)" ]]; then
    rm -rf "$dist"
    mkdir -p "$dist"
    cp -a "$previous/." "$dist/"
  fi
  rm -rf "$previous"
  return 0
}
trap cleanup EXIT

if [[ -d "$dist" ]]; then
  # `/.` so the contents land in $previous rather than a dist/ inside it.
  cp -a "$dist/." "$previous/"
fi

# A wipe, not an overwrite: a source file that stops emitting an asset must show
# up as a deletion, or a stale file would linger in the bundle forever. Safe
# because the snapshot above is put back if anything below this line fails.
rm -rf "$dist"
if ! npm run build; then
  echo >&2
  echo "the build failed, so the bundle already in the tree was left alone —" >&2
  echo "the crate still compiles. Fix the source error and run ui-build again." >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# Zero external requests (ADR-0078). The node must work with the machine
# offline, so nothing in the bundle may reach the network: no CDN script, no
# webfont, no remote image, no source map pointing at the internet.
# ---------------------------------------------------------------------------

# Exits, so the EXIT trap puts the bundle that was in the tree back: a rejected
# build is not a build, and leaving its output in dist/ would embed it.
fail() {
  echo >&2
  echo "web UI audit failed: $1" >&2
  echo "the bundle already in the tree was left alone." >&2
  exit 1
}

# Absolute URLs that are *strings*, never fetched, and are expected to be there.
# Everything else is a finding. Keep this list short and justified.
#   - http://www.w3.org/…      XML namespaces handed to createElementNS / in SVG
#   - https://svelte.dev/e/…   error-code doc links inside thrown message strings
allowed_url_prefixes='^(http://www\.w3\.org/|https://svelte\.dev/e/)'

if [[ ! -f "$dist/index.html" ]]; then
  fail "the build produced no dist/index.html"
fi

# The shell is the one file a browser loads by URL, so it gets the strict rule:
# no absolute or protocol-relative reference of any kind.
if grep -qE '(src|href)="(https?:)?//' "$dist/index.html"; then
  grep -nE '(src|href)="(https?:)?//' "$dist/index.html" >&2
  fail "dist/index.html references a remote URL"
fi

if find "$dist" -name '*.map' -print -quit | grep -q .; then
  fail "the build emitted source maps (set build.sourcemap = false)"
fi

if grep -rql 'sourceMappingURL' "$dist"; then
  fail "a bundled file points at a source map"
fi

if grep -rqE '@font-face|@import[[:space:]]+url\(|url\((https?:)?//' "$dist"; then
  grep -rnE '@font-face|@import[[:space:]]+url\(|url\((https?:)?//' "$dist" >&2
  fail "the bundle loads a font or stylesheet over the network"
fi

# Finally, every absolute URL anywhere in the bundle must be on the allowlist.
unexpected=$(grep -rohE 'https?://[A-Za-z0-9._~:/?#@!$&*+,;=%-]+' "$dist" |
  sort -u | grep -vE "$allowed_url_prefixes" || true)
if [[ -n "$unexpected" ]]; then
  echo "$unexpected" >&2
  fail "unexpected absolute URLs in the bundle (see the allowlist in this script)"
fi

# The bundle rides inside every copy of the binary, so keep the number visible.
bytes=$(find "$dist" -type f -printf '%s\n' | awk '{ total += $1 } END { print total+0 }')
files=$(find "$dist" -type f | wc -l)
echo "web UI bundle: $files files, $bytes bytes"

if ! $check_only; then
  # Built and audited: this bundle is the point of the run, so keep it.
  restore=false
  exit 0
fi

# ---------------------------------------------------------------------------
# Drift gate: compare the bundle that was in the tree against the one this
# rebuild just produced.
#
# Content, not `git status`. The invariant is "the bundle in the tree is what
# its sources compile to", and comparing bytes states exactly that, with no
# dependency on git at all. Asking git instead would answer a *different*
# question — "is the working tree clean?" — which happens to coincide only in
# CI, on a fresh checkout where the working tree is the committed tree. It goes
# wrong everywhere else, and the failures are the kind that get worked around:
#
#   - Nothing is committed yet. The very first `ui-build --check` on a branch
#     that adds the bundle reports drift no rebuild can fix, because the files
#     are new. Same for any tree with the bundle staged but not committed.
#   - `git` need not be git. A colocated jj checkout puts a shim on PATH that
#     ignores `-C` when resolving the pathspec, so the query silently widens to
#     the whole repo and every unrelated edit reads as bundle drift.
#
# Filenames are content-hashed, so a changed asset shows up as a deletion plus
# an addition rather than an edit — `diff -r` reports both, which is why this
# recurses over the trees instead of diffing file by file. `-q` because the
# payload is minified: which files differ is the useful signal, and dumping a
# unified diff of an 87 kB single-line bundle is not.
# ---------------------------------------------------------------------------
if ! drift=$(diff -rq "$previous" "$dist" 2>&1); then
  echo >&2
  # Name the two sides. Unsubstituted, every line reads as a temp directory
  # against an absolute path and it is not obvious which one is the committed
  # bundle and which one is what the sources actually compile to.
  drift=${drift//$previous/[committed]}
  echo "${drift//$dist/[rebuilt]}" >&2
  echo >&2
  echo "the committed web UI bundle does not match its sources." >&2
  echo "run \`ui-build\` and commit opys-server/ui/dist." >&2
  exit 1
fi
echo "the committed web UI bundle matches its sources"
