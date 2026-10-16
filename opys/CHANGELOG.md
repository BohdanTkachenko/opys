# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.13.0](https://github.com/BohdanTkachenko/opys/compare/v0.12.0...v0.13.0) - 2026-10-16

### Added

- *(verify)* restore `--fix` — opys acts on what it can compute
- *(server)* show what every branch holds, including the ones nobody checked out
- *(next)* answer which documents are pickable, and why
- *(apply)* one all-or-nothing write for a status, fields and a log entry
- *(order)* order documents by priority, what they block, and id
- *(verify)* observe time — stuck documents, revisited statuses, exit code 3
- *(lifecycle)* [**breaking**] derive blocked from live blockers instead of storing it
- *(git)* read the inventory on every branch, in process, by default
- *(log)* add `opys log`, and make log sections dated and checkable
- *(lifecycle)* let each type declare which status may follow which
- *(lifecycle)* record every status change in an auto-maintained history map
- *(ids)* allocate from interleaved sequences so branches never collide
- *(cli)* the `opys web` surface, systemd install, and M1 closeout (TASK-0075)
- *(core)* advisory inventory lock + id-allocation seam; retire the TUI

### Fixed

- *(server)* bind before scanning, and stop losing worktrees to a missing git
- land PR #17 at the repo root and untrack the committed worktree
- *(lock)* move the inventory lock out of the repo
- harden the retired ledger, retire, and query --write (four bugs)
- repair linkify, history, import, and renumber (four bugs)
- close five verify/serialization bugs (frontmatter, reason, unblock, agent-rules)
- *(nix,publish)* repair the flake build and restore crate READMEs

### Other

- make every page true of the shipped lifecycle, and runnable
- *(verify)* [**breaking**] observe staleness without acting on it
- *(query)* [**breaking**] make SQL read-only and give every write an id list
- *(ui)* ship the web UI bundle in the crate tarball, not in git
- release v0.12.0

## [0.12.0](https://github.com/BohdanTkachenko/opys/compare/v0.11.0...v0.12.0) - 2026-07-06

### Fixed

- *(publish)* drop readme from the opys binary manifest

### Other

- bump workspace to 0.12.0
- rename the library opys-core -> opys-engine
- share one workspace version; single unified release-plz release
- [**breaking**] rename opys->opys-core, opys-bin->opys; TUI behind an opt-in feature
