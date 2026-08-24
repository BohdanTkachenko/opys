# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.12.1](https://github.com/BohdanTkachenko/opys/compare/v0.12.0...v0.12.1) - 2026-08-24

### Added

- *(core)* advisory inventory lock + id-allocation seam; retire the TUI

### Fixed

- *(lock)* move the inventory lock out of the repo
- harden the retired ledger, retire, and query --write (four bugs)
- *(nix,publish)* repair the flake build and restore crate READMEs
