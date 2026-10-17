#!/usr/bin/env bash
# Exercise the build script in a checkout with no generated bundle.
set -euo pipefail
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
unset OPYS_UI_DIST CARGO_FEATURE_WEB_UI
rustc --edition=2021 opys-server/build.rs -o "$tmp/build-script"
export CARGO_MANIFEST_DIR="$tmp/clone" OUT_DIR="$tmp/out"
mkdir -p "$CARGO_MANIFEST_DIR" "$OUT_DIR" "$tmp/external bundle"
printf 'test bundle' > "$tmp/external bundle/index.html"

# No-UI builds remain supported without a bundle.
"$tmp/build-script" > "$tmp/log"
! grep -q 'cargo::error' "$tmp/log"
! grep -q 'include_bytes!' "$OUT_DIR/assets.rs"

export CARGO_FEATURE_WEB_UI=1
"$tmp/build-script" > "$tmp/log"
grep -q 'cargo::error=.*empty or missing' "$tmp/log"

# A temporary clone embeds the caller's bundle, including paths with spaces.
export OPYS_UI_DIST="$tmp/external bundle"
"$tmp/build-script" > "$tmp/log"
! grep -q 'cargo::error' "$tmp/log"
grep -q 'cargo::rerun-if-env-changed=OPYS_UI_DIST' "$tmp/log"
rustc --crate-type lib "$OUT_DIR/assets.rs" -o "$tmp/assets.rlib"
grep -q 'index.html' "$OUT_DIR/assets.rs"

# Normal packaged builds still use their own bundle without an override.
unset OPYS_UI_DIST
mkdir -p "$CARGO_MANIFEST_DIR/ui/dist"
cp "$tmp/external bundle/index.html" "$CARGO_MANIFEST_DIR/ui/dist/"
"$tmp/build-script" > "$tmp/log"
! grep -q 'cargo::error' "$tmp/log"
rustc --crate-type lib "$OUT_DIR/assets.rs" -o "$tmp/assets.rlib"
