# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.14.0](https://github.com/BohdanTkachenko/opys/compare/opys-server-v0.13.1...opys-server-v0.14.0) - 2026-10-17

### Added

- *(web)* add one-click type isolation to lifecycle graph
- *(web)* toggle graph types and keep navigation expanded
- *(web)* show the complete live project lifecycle on one board
- *(web)* redesign lifecycle as an ELK map with a document inspector
- *(web)* focus lifecycle arrows on the selected status
- *(verify)* seed missing history and expose scoped UI fixes

### Fixed

- *(web)* keep lifecycle inspector open and restore navigation toggle
- *(web)* highlight Only for the sole visible type
- *(web)* draw reciprocal transitions as one two-way connection
- *(web)* draw permitted lifecycle transitions as visible arrows
- *(web)* enable Fix only for repairable verification findings
- *(web)* highlight card verification problems
- *(web)* collapse verification issue details by default
- *(web)* include path-based verification errors on cards
- *(web)* display verification issue details on corpus boards

### Other

- *(web)* soften lifecycle arrows and highlight connected statuses

## [0.13.1](https://github.com/BohdanTkachenko/opys/compare/opys-server-v0.13.0...opys-server-v0.13.1) - 2026-10-17

### Added

- lifecycle, pick oracle, branch reader, flow view and block editor

### Fixed

- *(release)* supply the UI bundle to temporary semver checkouts

### Other

- *(changelog)* write the 0.13.0 release notes
- release v0.13.0

## [0.13.0](https://github.com/BohdanTkachenko/opys/compare/opys-server-v0.12.0...opys-server-v0.13.0) - 2026-10-16

### Added

- First release of the always-on node behind `opys web`: it serves the
  allowlisted projects through a read API with WebSocket events, and writes
  through the engine's own command cores.
- The embedded web UI: a keyboard-driven board, the document view with
  in-place block editing, the query console, the union of a project's
  worktrees and branches, the lifecycle flow graph, and an omnibox. It installs
  as a web app.
- Allowlist management from the browser, and a systemd user unit
  (`opys web install`).
- `GET /api/projects/{key}/next` and `/flow`.

### Fixed

- The node binds its socket before scanning, and finds worktrees even when the
  service's `PATH` has no `git`.
