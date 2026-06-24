# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.10.0](https://github.com/BohdanTkachenko/opys/compare/v0.9.0...v0.10.0) - 2026-06-24

### Added

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
