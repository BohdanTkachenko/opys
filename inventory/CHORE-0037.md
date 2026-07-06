---
id: CHORE-0037
status: todo
created: "2026-07-06T05:14:48Z"
updated: "2026-07-06T05:14:48Z"
references:
  FEAT-0022: Multi-agent skill and plugin packaging
---

# Fix flake.nix workspace version — nix run is broken

## Tasks
- [ ] change `version = cargoToml.package.version` to `cargoToml.workspace.package.version` in flake.nix
- [ ] `nix eval .#packages.x86_64-linux.default.version` returns 0.12.0
- [ ] consider building the flake package with the tui feature since the README positions it as an install path

## Progress
- Filed from the pre-announcement review: since the workspace-version refactor, `[package] version.workspace = true` makes the flake read the attrset `{ workspace = true; }` — `nix run github:BohdanTkachenko/opys` (advertised in the README) fails to evaluate.
