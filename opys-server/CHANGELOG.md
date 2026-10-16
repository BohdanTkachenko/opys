# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.13.0](https://github.com/BohdanTkachenko/opys/compare/opys-server-v0.12.0...opys-server-v0.13.0) - 2026-10-16

### Added

- svelte-blockdown, a reusable block-level markdown editor and diff; edit card bodies in place
- *(ui)* double-click to edit a document body, click outside to save
- *(ui)* a card-stack app icon, with the SVG as its source
- *(ui)* make the dashboard an installable web app
- *(ui)* fold the wide-screen sidebar into a rail; plan card history and comment threads
- *(server)* draw the lifecycle as a graph with the documents standing on it
- *(server)* show what every branch holds, including the ones nobody checked out
- *(next)* answer which documents are pickable, and why
- *(apply)* one all-or-nothing write for a status, fields and a log entry
- *(verify)* observe time — stuck documents, revisited statuses, exit code 3
- *(lifecycle)* [**breaking**] derive blocked from live blockers instead of storing it
- *(git)* read the inventory on every branch, in process, by default
- *(log)* add `opys log`, and make log sections dated and checkable
- *(lifecycle)* let each type declare which status may follow which
- *(lifecycle)* record every status change in an auto-maintained history map
- *(ids)* allocate from interleaved sequences so branches never collide
- *(ui)* keyboard-first board and the omnibox
- *(ui)* the terminal-modern redesign
- *(server)* manage the allowlist from the browser (FEAT-0083, ADR-0082)
- *(cli)* the `opys web` surface, systemd install, and M1 closeout (TASK-0075)
- *(server)* embedded Svelte web UI and its build pipeline (TASK-0074)
- *(server)* worktree union view (TASK-0073)
- *(server)* write actions through the engine command cores (TASK-0072)
- *(server)* read API, WebSocket events, and the main loop (TASK-0071)
- *(server)* corpus actor with warm store and watcher (TASK-0070)
- *(server)* allowlist registry and bounded project discovery (TASK-0069)
- *(server)* scaffold opys-server — the always-on node (TASK-0067)

### Fixed

- *(ui)* returning to the board keeps its filters
- *(server)* bind before scanning, and stop losing worktrees to a missing git
- land PR #17 at the repo root and untrack the committed worktree

### Other

- make every page true of the shipped lifecycle, and runnable
- *(query)* [**breaking**] make SQL read-only and give every write an id list
- *(ui)* ship the web UI bundle in the crate tarball, not in git
- move the commercial roadmap out, and sweep what referenced it
- *(inventory)* ADR-0076/0077 — Apache node, allowlist over scan
