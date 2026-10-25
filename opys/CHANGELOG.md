# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.15.0](https://github.com/BohdanTkachenko/opys/compare/v0.14.0...v0.15.0) - 2026-10-25

### Added

- *(roles)* declare human agent and automation worker kinds
- *(core)* allow shared work stages and reject conflicting branch kinds
- *(core)* resolve declared work branches and claims in engine and CLI

### Fixed

- *(core)* validate work copy identity and status
- *(core)* hold unreadable work copies as unavailable
- *(core)* preserve work occupancy across aliases and checkouts

## [0.14.0](https://github.com/BohdanTkachenko/opys/compare/v0.13.1...v0.14.0) - 2026-10-20

### Added

- *(server)* serve project and cross-project role queues
- *(queue)* expose role picks and shared attention queues
- *(verify)* report source ranges and highlight validation failures
- *(verify)* seed missing history and expose scoped UI fixes

### Fixed

- *(queue)* align malformed-file findings with node paths

## [0.13.1](https://github.com/BohdanTkachenko/opys/compare/v0.13.0...v0.13.1) - 2026-10-17

### Other

- a short contributor guide, and the upgrade steps in the README

## [0.13.0](https://github.com/BohdanTkachenko/opys/compare/v0.12.0...v0.13.0) - 2026-10-16

### Added

- `opys web`: the always-on node and its embedded web UI, built into the CLI.
- `opys next`, `opys apply`, `opys log` and `opys history`, and `verify --fix`
  (see opys-engine).

### Changed

- **Breaking:** `opys query` is read-only, and there is no stored `blocked`
  status (see opys-engine).
- The terminal UI is retired in favour of the web UI.

### Fixed

- The inventory lock lives outside the repository.
- The Nix flake build and the crate READMEs.

## [0.12.0](https://github.com/BohdanTkachenko/opys/compare/v0.11.0...v0.12.0) - 2026-07-06

### Fixed

- *(publish)* drop readme from the opys binary manifest

### Other

- bump workspace to 0.12.0
- rename the library opys-core -> opys-engine
- share one workspace version; single unified release-plz release
- [**breaking**] rename opys->opys-core, opys-bin->opys; TUI behind an opt-in feature
