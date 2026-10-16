# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.13.0](https://github.com/BohdanTkachenko/opys/compare/v0.12.0...v0.13.0) - 2026-10-16

### Added

- *(git)* read the inventory on every branch, in process, by default
- *(lifecycle)* record every status change in an auto-maintained history map
- *(ids)* allocate from interleaved sequences so branches never collide
- *(core)* advisory inventory lock + id-allocation seam; retire the TUI

### Fixed

- *(lock)* move the inventory lock out of the repo
- harden the retired ledger, retire, and query --write (four bugs)
- *(nix,publish)* repair the flake build and restore crate READMEs
