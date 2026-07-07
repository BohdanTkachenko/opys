# opys

File-based inventory of typed markdown documents for human + AI codebases — one
markdown file per document, verified in CI. This crate is the **`opys` CLI** (the
binary you get from `cargo install opys`).

`opys` manages a version-controlled inventory of *what a product does*: one
markdown file per document, each with YAML frontmatter (stable `PREFIX-NNNN` id,
status, tags, relation maps) plus a markdown body. Document **types** — their id
prefixes, statuses, custom fields, required sections, and validation rules — are
configured in one `opys.toml`. All writes go through the CLI so invariants hold
at write time and parallel agents don't collide; reads are plain `grep` + targeted
file reads. `opys verify` is the CI gate. It is deliberately *not* a task board —
no sprints, assignees, or priorities.

```sh
cargo install opys                 # the CLI (what agents use)
cargo install opys --features tui  # + the interactive `opys tui` terminal board
```

```sh
opys init                          # scaffold opys.toml + the inventory dir
opys new --type feature --title "Tab title follows OSC 0/2" --tags osc,tabs
opys list --type feature --status implemented
opys verify                        # wire into CI
```

## Workspace

- **`opys`** — this crate: the command-line binary.
- [`opys-engine`](https://crates.io/crates/opys-engine) — the core library (model, config, rules, SQL store, command implementations).
- [`opys-backend-markdown-local`](https://crates.io/crates/opys-backend-markdown-local) — the default storage backend (one markdown file per document on the local filesystem).
- [`opys-tui`](https://crates.io/crates/opys-tui) — the optional `opys tui` terminal board (behind the `tui` feature).

Full documentation, the format spec, and the agent workflow live in the
[project README](https://github.com/BohdanTkachenko/opys).

Licensed under Apache-2.0.
