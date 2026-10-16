# Contributing to opys

Guidance for people and coding agents working in this repository. Claude Code
reads it through `CLAUDE.md`; other tools read `AGENTS.md` directly.

## What this is

`opys` is a Rust CLI that manages a file-based inventory of typed markdown
documents: one file per document, with YAML frontmatter (a stable `PREFIX-NNNN`
id, status, tags, relation maps) and a markdown body. Every write goes through
the CLI, so invariants hold at write time; reads are plain files. `opys verify`
is the CI gate. Everything is configured by one `opys.toml` at the project root.

The normative spec is [`skills/opys/references/format.md`](skills/opys/references/format.md).
Read it before changing parsing, serialization or `verify`, and keep it in sync
with the code.

## Layout

A Cargo workspace of four crates, published together under one version:

- **`opys-engine`** (repo root, `src/`) — the model, config, rules, the
  in-memory SQL store and every command. It never depends on `opys-server`.
- **`opys-backend-markdown-local`** — the default storage backend: markdown
  files on disk, plus the inventory lock.
- **`opys-server`** — the always-on node behind `opys web`: HTTP/WebSocket API
  and the embedded Svelte web UI (`opys-server/ui/`).
- **`opys`** — the binary that joins the CLI and `opys web`.

`packages/blockdown` is the block editor the web UI uses, as a standalone
Svelte package.

## Build, test, lint

The devShell (`nix develop`, or direnv) provides the toolchain. CI runs:

```sh
scripts/ui-build.sh                 # build opys-server/ui/dist first: it is embedded, not committed
npm --prefix opys-server/ui test
npm test --prefix packages/blockdown
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --all --all-features
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo test --all --no-default-features   # the no-Node path: builds without the UI
scripts/sync-versions.sh --check
```

A fresh checkout does not compile with default features until the UI bundle
is built; `--no-default-features` is the supported way to work on the Rust
without Node. The minimum Rust version is 1.88 (`msrv` in the devShell checks
it) — don't use newer std APIs.

## Conventions

- **Errors vs findings.** A failure that should abort a command is an
  `OpysError` (exit 2). A content problem `verify` finds is collected and
  printed, never raised, so every problem is reported at once. `verify` exits
  `1` for problems and `3` when all it found are time observations.
- **Canonical frontmatter.** Serialization is canonical and pinned by unit tests
  in `src/frontmatter.rs`; change the output only deliberately.
- **Tests.** Integration tests drive the built binary over a temporary project
  (`opys/tests/cli.rs`, `opys/tests/web.rs`); unit tests sit next to the code.
- **Licence.** Every crate is Apache-2.0, permanently (ADR-0033).

## This repo's own inventory

The repo tracks its features and decisions with opys, under `inventory/`. It
declares only the two permanent types, `feature` and `adr`. Find documents with
`opys list` or `rg`; change them through the CLI (`new --type`, `set-status`,
`tag`, `block`), and run `opys verify` before you finish. Accepted ADRs are
settled: supersede one with a new ADR rather than editing it.
