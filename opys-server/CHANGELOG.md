# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.15.0](https://github.com/BohdanTkachenko/opys/compare/opys-server-v0.14.0...opys-server-v0.15.0) - 2026-10-23

### Added

- *(web)* distinguish empty and blocked lifecycle backgrounds
- *(web)* show lifecycle totals with blocked badges
- *(web)* show unblocked over total lifecycle counts
- *(web)* show claimed cards on the lifecycle
- *(web)* display claimed work branches and protect branch edits
- *(ui)* show branch ticket versions in the lifecycle view
- *(flow)* expose labelled progress from branches and worktrees

### Fixed

- *(server)* serve cached reads while inventories reload
- *(server)* keep inventory reads responsive and config watches current

### Other

- *(web)* budget lifecycle claim activity in the bundle

## [0.14.0](https://github.com/BohdanTkachenko/opys/compare/opys-server-v0.13.1...opys-server-v0.14.0) - 2026-10-20

### Added

- *(ui)* add role queues and status owner views
- *(server)* serve project and cross-project role queues
- *(queue)* expose role picks and shared attention queues
- *(web)* explain read-only configuration and disable edits
- *(web)* show discovered projects in the sidebar
- *(web)* highlight exact validation ranges in the markdown editor
- *(verify)* report source ranges and highlight validation failures
- *(web)* zoom lifecycle canvas with Ctrl and mouse wheel
- *(web)* use lifecycle canvas with docked status pane
- *(web)* streamline lifecycle header and graph footer
- *(web)* add one-click type isolation to lifecycle graph
- *(web)* toggle graph types and keep navigation expanded
- *(web)* show the complete live project lifecycle on one board
- *(web)* redesign lifecycle as an ELK map with a document inspector
- *(web)* focus lifecycle arrows on the selected status
- *(verify)* seed missing history and expose scoped UI fixes

### Fixed

- *(web)* report registration results and refresh projects immediately
- *(web)* show only drifted entries in project comparisons
- *(web)* show checkbox save refusals beside the affected source
- *(web)* validate drafts while editing markdown
- *(web)* handle lazy layout assets in caching and bundle checks
- *(web)* align markdown task checkboxes and remove duplicate bullets
- *(web)* wrap lifecycle type filters onto new lines
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

- *(web)* unify checkbox appearance with site controls
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
