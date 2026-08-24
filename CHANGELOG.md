# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.12.1](https://github.com/BohdanTkachenko/opys/compare/v0.12.0...v0.12.1) - 2026-08-24

### Added

- *(server)* embedded Svelte web UI and its build pipeline (TASK-0074)
- *(server)* worktree union view (TASK-0073)
- *(server)* write actions through the engine command cores (TASK-0072)
- *(server)* read API, WebSocket events, and the main loop (TASK-0071)
- *(server)* corpus actor with warm store and watcher (TASK-0070)
- *(server)* allowlist registry and bounded project discovery (TASK-0069)
- *(server)* scaffold opys-server — the always-on node (TASK-0067)
- *(core)* advisory inventory lock + id-allocation seam; retire the TUI

### Fixed

- *(lock)* move the inventory lock out of the repo
- harden the retired ledger, retire, and query --write (four bugs)
- repair linkify, history, import, and renumber (four bugs)
- close five verify/serialization bugs (frontmatter, reason, unblock, agent-rules)
- *(nix,publish)* repair the flake build and restore crate READMEs
- *(tui)* harden board writes and close BUG-0031

### Other

- *(inventory)* close TASK-0074; TASK-0075 becomes pickable
- *(inventory)* close TASK-0073; TASK-0074 becomes pickable
- *(inventory)* close TASK-0072
- *(inventory)* close TASK-0071; TASK-0072 and TASK-0073 become pickable
- *(inventory)* ADR-0078 — Svelte for the web UI, committed build output
- *(inventory)* close TASK-0070; TASK-0071 becomes pickable
- *(inventory)* close TASK-0069; TASK-0070 becomes pickable
- *(inventory)* close TASK-0068 as a no-op; its premise was already true
- *(inventory)* accept ADR-0077 — background scan, depth 10, allowlist
- *(inventory)* ADR-0076/0077 — Apache node, allowlist over scan
- *(inventory)* close TASK-0067; TASK-0069 becomes pickable
- *(inventory)* plan M1 — nine spec-complete tasks for opys-server
- *(inventory)* record ADR-0066 — SQL as the internal working representation
- *(inventory)* accept ADR-0056 (Apache core, AGPL server components)
- *(inventory)* file the platform pivot — adr type, 8 ADRs, 7 platform features
- *(tui)* cut the built-in editor for a read-mostly board
- dogfood opys — seed the repo's own feature inventory

## [0.11.0](https://github.com/BohdanTkachenko/opys/compare/v0.10.1...v0.11.0) - 2026-07-04

### Added

- *(query)* read-only `opys query "SELECT …"` over the corpus store
- *(store)* in-memory SQL corpus store — load, decompose, reconstruct, flush
- replace hardcoded stats with config-driven SQL `[[stats]]`

### Fixed

- *(msrv)* correct floor to 1.88 and make it locally verifiable
- *(mdprism)* make the markdown⇄structure round-trip lossless

### Other

- document the store & `opys query`; remove corpus spike
- *(renumber,verify)* port to the store; drop dead project helpers
- *(sync)* run the auto-sync pass over the store
- *(relations)* port block/unblock/close/cleanup to the store
- *(mutators)* port new/import/set-status/tag/retire to the store
- *(reads)* port list/tags/show to the SQL corpus store

## [0.10.1](https://github.com/BohdanTkachenko/opys/compare/v0.10.0...v0.10.1) - 2026-07-03

### Fixed

- linkify no longer re-linkifies inside existing markdown links; verify flags nested links

## [0.10.0](https://github.com/BohdanTkachenko/opys/compare/v0.9.0...v0.10.0) - 2026-06-24

### Added

- track code references to ids (show --refs, renumber warning)
- add `opys renumber` to resolve cross-branch ID conflicts
- section-kind-driven coverage stats
- *(mdprism)* add render (data→md) and query (jq via jaq)
- structured sections validated by mdprism (replace [[parts]])
- *(mdprism)* validate() — body conformance via comrak
- *(mdprism)* workspace + crate skeleton with the schema DSL parser
- configurable structured section kind; per-section stats coverage

### Fixed

- satisfy clippy::unnecessary_sort_by in renumber

### Other

- *(skill)* document renumber and show --refs in the agent rule and skill
- inline mdprism as a module so opys can publish to crates.io
- release via GitHub only, never publish to crates.io
- rustfmt
- update for structure-based structured sections
- delimited <? ?> descriptions + escaping rules
- cardinality leads the head (before @name), consistent column
- bare literal labels; glue cardinality to marker/name
- @name leads the element; clarify markdown coverage
- add mdprism kitchen-sink reference (every feature demonstrated)
- clarify @name as a rename-proof block alias
- name the crate mdprism
- reframe DSL crate as a bidirectional markdown<->data codec
- expand DSL spec — captures, descriptions, query, in-place edit
- markdown structure DSL design spec (mdrubric, working name)
- expose the opys package, app, and overlay from the flake

## [0.9.0](https://github.com/BohdanTkachenko/opys/compare/v0.8.0...v0.9.0) - 2026-06-20

### Added

- tag breakdown in stats and an `opys tags` command

## [0.8.0](https://github.com/BohdanTkachenko/opys/compare/v0.7.0...v0.8.0) - 2026-06-20

### Added

- structured tags (colon/equals), key search, tag rule guards

## [0.7.0](https://github.com/BohdanTkachenko/opys/compare/v0.6.0...v0.7.0) - 2026-06-19

### Added

- accept multiple ids for bulk mutations

### Other

- exercise the `history` feature; test the command end-to-end
- automate releases with release-plz
- Add optional `opys history <id>` command (gix-backed)
