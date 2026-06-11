# Agent Instructions

This file provides guidance to AI coding assistants (Claude Code and any other AGENTS.md-aware tool) when working with code in this repository.

## What this is

`opys` is a Rust CLI that manages a **file-based feature inventory**: one
markdown file per feature, each with `---`-fenced YAML frontmatter (stable ID,
status, tags) plus a markdown body (spec prose, a `## Test plan`, a
`## Manual verification` section). All writes go through the CLI so invariants
hold at write time and parallel agents don't collide; reads are plain `grep` +
targeted file reads. `verify` is the CI gate. It is deliberately *not* a task
board — no sprints, assignees, or priorities.

The tool ships alongside the `feature-inventory` skill, single-sourced in
`.rulesync/skills/feature-inventory/`. The normative format spec lives at
`.rulesync/skills/feature-inventory/references/format.md` — consult it before
changing parsing, serialization, or `verify` semantics, and keep the two in
sync.

The per-tool skill copies under `.claude/`, `.cursor/`, and `.agents/` are
**generated** from that source with `rulesync` (targets pinned in
`rulesync.jsonc`) — never hand-edit them. After editing `.rulesync/`, run
`npx rulesync@8 generate`; CI runs `npx rulesync@8 generate --check` to fail on
stale output.

## Development Environment

This project uses a Nix flake with a devShell (`flake.nix`) and direnv
(`.envrc`), which provide the Rust toolchain (`cargo`, `rustc`, `clippy`,
`rustfmt`, `rust-analyzer`).

To add a new tool, add it to `devPackages` in the devShell in `flake.nix` and
run `refresh`. Do not use `nix run` or `nix shell` for project tooling — keep
everything in the devShell. Use `nix run` only for one-off commands that don't
belong in the devShell permanently.

## Build / test / lint

The CI that gates merges (`.github/workflows/ci.yml`) runs exactly:

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings   # warnings are errors
cargo test --all
cargo build --all-targets                   # also built on MSRV 1.88 — don't use newer std APIs
```

Run a single test:

```sh
cargo test --test cli new_allocates_next_id_and_requires_tags   # one integration test
cargo test --lib frontmatter::                                  # unit tests in a module
```

## Architecture

The binary (`src/main.rs`) is a thin wrapper: it parses `Cli` (clap derive,
`src/cli.rs`) and calls `opys::run`, which maps the exit code. Everything is
exposed as a library (`src/lib.rs`) so the crate is usable as a dependency.

**Exit-code contract (important):** `verify` returns `1` when it finds content
problems; every other command returns `0` on success. Real failures (bad
flags, IO, missing config) surface as `OpysError` and the binary maps them to
exit `2`. Crucially, **content problems found by `verify` are not `OpysError`s**
— they are collected into a `Vec<String>` and printed, so verify can report
*all* problems at once rather than aborting on the first.

Layering, roughly outermost-in:

- `src/cli.rs` + `src/lib.rs` — `Cli`/`Command` enums, the dispatch `match`,
  and the `Ctx` struct (root dir, inventory `dir`, `no_sync` flag).
- `src/commands/` — one module per subcommand, each a `run(ctx, …)` fn.
  `commands/mod.rs` holds shared helpers: `today()`, `split_csv()`, and
  `maybe_sync()` (the auto-regeneration hook).
- `src/project.rs` — `Project` ties together the on-disk layout and config.
  `Project::open` requires `<base>/features/_config.toml`. Owns feature
  discovery (`feature_paths`, `load`), ID allocation (`next_id`), retired-ID
  tracking, and the shared regexes (`id_format_re`, `KEBAB_RE`, `parse_field`).
- `src/feature.rs` / `src/frontmatter.rs` / `src/body.rs` — the parse layer.
  `Feature::parse` splits frontmatter from body; `frontmatter` parses YAML with
  `serde_norway` and re-serializes canonically; `body` extracts the title,
  `## Test plan` checkbox items, and `## Manual verification` structured items.
- `src/config.rs` — `_config.toml` model (`prefix`, `pad`, `test_search_paths`,
  `test_reference_check`, custom `[fields.*]`, etc.) and the status list.

### Invariants enforced on disk (the point of the tool)

- **IDs**: `PREFIX-NNNN`, monotonic, never reused. `next_id` takes the max over
  both live *and* retired IDs. `retire` appends to `features/_retired.txt`;
  `verify` rejects any reuse.
- **Status lifecycle** (`config::CORE_STATUSES`): `planned`, `partial`,
  `implemented`, `wontfix`, plus configured `extra_statuses`. Guards live in
  `set_status`: `wontfix` requires a reason; `implemented` requires at least one
  checked `## Test plan` item. `verify` re-checks these independently.
- **Test references**: backticked refs (`` `mod::test_name` ``) on checked
  test-plan items must resolve. `verify`'s `TestIndex` does this in one of three
  modes from `test_reference_check`: `grep` (substring across
  `test_search_paths`), `extract` (regex-extract real names via
  `test_name_pattern`), or `none`.
- **Frontmatter is closed**: only `RESERVED_FIELDS` (`frontmatter.rs`) plus
  fields declared in `[fields.*]` are allowed; unknown keys fail `verify`.
  Declared custom fields are type-checked.

### Generated artifacts — never hand-edit

`features/INDEX.md` and everything under `views/` (`by-tag/`, `status/`) are
regenerated by `sync_views::regenerate`. Mutating commands (`new`,
`set-status`, `tag`, `retire`) call `maybe_sync` automatically unless
`--no-sync` is passed; `sync-views` rebuilds them after hand edits.
Regeneration refuses to run if any feature fails to parse (run `verify` first).

### Frontmatter serialization

`frontmatter::serialize` emits canonical output: core fields (`id`, `status`,
`tags`) first, remaining keys alphabetically; flat scalars and scalar lists
inline (`tags: [a, b]`), complex values as block YAML. `format_string` quotes
only when needed for unambiguous round-tripping. The unit tests in
`frontmatter.rs` pin this exact output — update them deliberately when changing
formatting.

## Conventions

- Errors that should abort a command are `OpysError` (`src/error.rs`); content
  problems for `verify` are pushed onto an error `Vec` instead. Keep that
  distinction — don't turn verify findings into hard errors.
- Reach for the inline-scalar / block-YAML split in `frontmatter.rs` rather than
  formatting YAML by hand elsewhere.
- Integration tests (`tests/cli.rs`) drive the built binary with `assert_cmd`
  over a `tempfile` project; unit tests live next to the code they cover.
