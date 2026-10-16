# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
