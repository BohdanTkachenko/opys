#!/usr/bin/env bash
# Sync the version of every packaging manifest to the crate version.
#
# The crate version in Cargo.toml ([package].version) is the single source of
# truth. The multi-agent packaging manifests (Claude/Codex plugins, the Gemini
# extension, the pi/npm package) each carry their own `version` field that must
# match, but are excluded from the crate, so nothing keeps them in lockstep
# automatically. This script does.
#
# Usage:
#   sync-versions          # rewrite manifests to match Cargo.toml
#   sync-versions --check   # report drift and exit non-zero (CI gate); no writes
set -euo pipefail

# Resolve the repo root from this script's location so it works from any cwd.
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$script_dir/.." && pwd)

check_only=false
if [[ "${1:-}" == "--check" ]]; then
  check_only=true
elif [[ $# -gt 0 ]]; then
  echo "usage: sync-versions [--check]" >&2
  exit 2
fi

# Source of truth: the version inside [package] of Cargo.toml.
crate_version=$(awk -F' *= *' '
  /^\[/        { section = $0 }
  section == "[package]" && $1 == "version" {
    gsub(/"/, "", $2); print $2; exit
  }
' "$root/Cargo.toml")

if [[ -z "$crate_version" ]]; then
  echo "could not read [package].version from Cargo.toml" >&2
  exit 2
fi

# JSON manifests that carry a top-level "version" to keep in sync.
manifests=(
  "package.json"
  "gemini-extension.json"
  ".claude-plugin/plugin.json"
  ".codex-plugin/plugin.json"
)

ver_re='("version"[[:space:]]*:[[:space:]]*")[^"]*(")'
drift=0
changed=0

for rel in "${manifests[@]}"; do
  file="$root/$rel"
  if [[ ! -f "$file" ]]; then
    echo "missing manifest: $rel" >&2
    drift=1
    continue
  fi

  current=$(grep -oE "\"version\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" "$file" | head -n1 | sed -E 's/.*"([^"]*)"$/\1/')

  if [[ "$current" == "$crate_version" ]]; then
    continue
  fi

  if $check_only; then
    echo "drift: $rel is $current, expected $crate_version"
    drift=1
  else
    # Edit only the version line so the diff stays minimal (no reformatting).
    sed -i -E "s/$ver_re/\1$crate_version\2/" "$file"
    echo "updated: $rel $current -> $crate_version"
    changed=$((changed + 1))
  fi
done

if $check_only; then
  if [[ $drift -ne 0 ]]; then
    echo "version drift detected (crate is $crate_version); run sync-versions" >&2
    exit 1
  fi
  echo "all manifests match crate version $crate_version"
else
  if [[ $changed -eq 0 ]]; then
    echo "already in sync at $crate_version"
  fi
fi
