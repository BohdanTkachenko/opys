# Agent Instructions

This file provides guidance to AI coding assistants (Claude Code and any other AGENTS.md-aware tool) when working with code in this repository.

## What this is

`opys` is a Rust CLI that manages a **file-based feature inventory**: one
markdown file per feature, each with `---`-fenced YAML frontmatter (stable ID,
status, tags) plus a markdown body (spec prose, a `## Test plan`, a
`## Manual verification` section). All writes go through the CLI so invariants
hold at write time and parallel agents don't collide; reads are plain `grep` +
targeted file reads. `verify` is the CI gate. The inventory base dir defaults to
`docs/opys/`. It is deliberately *not* a task board — no sprints, assignees, or
priorities.

It also manages an optional **work-item** subsystem (`opys work-item …`, alias
`wi`): ephemeral, per-change companion files in `docs/opys/work-items/` that come
in hardcoded *types* (`task`/`bug`/`chore` → `TASK-`/`BUG-`/`CHORE-NNNN`, the
type derived from the id prefix), with a `## Tasks` checklist and `## Progress`
log, that must reference ≥1 feature and are deleted on `close` (which strikes the
reference through as a tombstone). Features and work items share one uniform `references:`
ID→title map that `opys` keeps bidirectional, title-fresh, and linkified
automatically on every write.

The tool ships alongside the tool-agnostic `opys` skill in
`skills/opys/`. The normative specs live at
`skills/opys/references/format.md` (features) and
`references/work-items.md` (work items) — consult them before changing parsing,
serialization, or `verify` semantics, and keep all three (code ↔ format.md ↔
work-items.md) in sync. The README explains how users copy that one folder into
their tool's skills directory (`.claude/skills/`, `.cursor/skills/`,
`.agents/skills/`).

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
  `Project::open` requires `<base>/features/_config.toml` and optionally loads
  `<base>/work-items/_config.toml` (`wi_cfg: Option<…>`). Owns feature and
  work-item discovery (`feature_paths`/`load`, `work_item_paths`/
  `load_work_items`), ID allocation (`next_id`, `next_id_for_prefix`), retired-ID
  tracking (`read_id_ledger`/`write_id_ledger_entry`), and the shared regexes
  (`id_format_re`, `KEBAB_RE`, `parse_field`).
- `src/feature.rs` / `src/work_item.rs` / `src/frontmatter.rs` / `src/body.rs` —
  the parse layer. `Feature::parse` / `WorkItem::parse` split frontmatter from
  body; `frontmatter` parses YAML with `serde_norway` and re-serializes
  canonically; `body` extracts the title, checkbox items (`checklist_items`,
  used for `## Test plan` and `## Tasks`), and `## Manual verification` items.
- `src/refs.rs` — the uniform `references` ID→title map: parse/serialize (sorted
  by item number), strikethrough tombstone helpers, `id_number`.
- `src/links.rs` — the auto-sync engine: `reconcile`/`reconcile_blockers`
  (bidirectional, title-fresh relation maps between live docs) and `linkify`
  (bare feature/work-item ID mentions in prose → markdown links, skipping code;
  the prefix alternation is built from the live prefixes). Driven by
  `commands/sync.rs`, which `maybe_sync` calls.
- `src/commands/work_item/` — the `opys work-item …` subcommands.
- `src/config.rs` — `Config` (features: `pad`, `test_search_paths`,
  `test_reference_check`, custom `[fields.*]`) and `WorkItemConfig` (work items:
  `pad`, `extra_statuses`, `required_sections`, `[fields.*]`); the status lists;
  the fixed `FEAT_PREFIX`; and the hardcoded `WORK_ITEM_TYPES` table
  (`task`/`bug`/`chore`, each a name + prefix + per-type extra sections) with
  `type_for_id`/`type_by_name`/`is_work_item_id`/`work_item_prefixes`.

### Invariants enforced on disk (the point of the tool)

- **IDs**: fixed prefixes (`FEAT-NNNN`; work items `TASK-`/`BUG-`/`CHORE-NNNN`),
  monotonic per prefix, never reused. `next_id` takes the max over live *and*
  retired features; `next_id_for_prefix` takes the max for a given work-item
  prefix over live work items *and* every id of that prefix appearing in any
  relation map (`references`/`blocked_by`/`blocks`, struck or not —
  `refs::all_ids_with_prefix`), so a closed work item's tombstone reserves its id
  and each type has an independent sequence. `retire` appends to
  `features/_retired.txt` (kept sorted); `verify` rejects reuse.
- **References** (`references` map, both families): the uniform ID→title link
  field is auto-reconciled on every write (`links::reconcile`) — bidirectional
  between live docs, titles refreshed from the target, sorted by number. A
  closed work item leaves a struck-through (`~~title~~`) tombstone. `verify`
  fails only on a non-struck id that resolves to nothing, or a work item that
  references no live feature; title drift / missing reverse links are auto-fixed,
  not gated. Bare feature/work-item ID mentions in body prose are linkified
  (`links::linkify`), skipping code spans/fences.
- **Blockers** (`blocked_by` / `blocks` maps, both families): a directional
  relation built on the same ID→title machinery as `references`. `opys block
  <id> --by <id>` / `unblock` write `blocked_by` on the blocked side and the
  inverse `blocks` on the blocker (`links::reconcile_blockers` keeps them
  inverse and title-fresh). Blocking a *work item* auto-sets it to `blocked`
  (the link satisfies the `blocked_reason` guard); `unblock` reverts it to
  `in-progress` when no blocker/reason remains. Entries resolve, tombstone on
  close, and reserve ids exactly like references; a map may not list itself
  (`refs::RELATION_FIELDS` drives close/cleanup/verify/id-reservation uniformly).
- **Status lifecycle** (`config::CORE_STATUSES`): `planned`, `partial`,
  `implemented`, `wontfix`, plus configured `extra_statuses`. The guards
  (`wontfix` requires a reason; `implemented` requires at least one checked
  `## Test plan` item; unknown statuses rejected) are enforced at *every* write
  point — `set_status`, `new`, and `import` — and `verify` re-checks them
  independently. (`new` can never be `implemented`: a fresh file has no test
  plan, so it is rejected outright rather than deferred to verify.)
- **Test references**: a backtick span is a test reference only when it
  contains `::` (`` `mod::test_name` ``); prose code spans on a checked item are
  ignored (`body::is_test_ref`). Referenced tests must resolve — `verify`'s
  `TestIndex` does this in one of three modes from `test_reference_check`:
  `grep` (substring across `test_search_paths`), `extract` (regex-extract real
  names via `test_name_pattern`), or `none`.
- **Work-item lifecycle**: every work-item *type* (`WORK_ITEM_TYPES`) shares one
  lifecycle. `WI_CORE_STATUSES` = `todo`/`in-progress`/`blocked`/`done`. `done`
  is terminal and reached only via `close` (`new`/`set-status` reject it;
  `blocked` requires `blocked_reason` *or* a `blocked_by` link). Required body
  sections — the configured `required_sections` baseline ∪ the type's
  `extra_required_sections` (e.g. `bug` → `## Reproduction`), via
  `WorkItemType::required_sections` — and the ≥1-feature link are enforced at
  write time and re-checked by `verify`. `verify` also rejects an unrecognized
  id prefix (an unknown type). `close` refuses unchecked tasks (unless
  `--force`), deletes the file, and strikes the reference through everywhere;
  `cleanup` strips struck references.
- **Frontmatter is closed**: only `RESERVED_FIELDS` / `WI_RESERVED_FIELDS`
  (`frontmatter.rs`, which include `references`/`blocked_by`/`blocks`) plus
  fields declared in `[fields.*]` are allowed; unknown keys fail `verify`.
  Declared custom fields are type-checked (`check_custom_fields`, shared between
  both families); a `type = "enum"` field additionally constrains the value to
  its declared `values` set, and `list`/`work-item list` can filter by any
  custom field with repeatable `--field key=value`.

### Generated artifacts — never hand-edit

`features/INDEX.md`, `work-items/INDEX.md`, and everything under `views/`
(`by-tag/`, `status/`, and `wi-by-feature/`, `wi-status/` when work items are
configured) are regenerated by `sync_views::regenerate`. Mutating commands
(`new`, `set-status`, `tag`, `retire`, and the `work-item …` mutators) call
`maybe_sync` → `commands/sync::run` automatically unless `--no-sync` is passed;
that pass also reconciles references and linkifies prose before regenerating
views. `sync-views` rebuilds everything after hand edits. Regeneration refuses
to run if any feature or work item fails to parse (run `verify` first).

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

## Multi-agent packaging

The repo is also a multi-agent plugin for the `opys` skill. The
`skills/opys/` folder is the conditional skill (shipped by the
Claude Code plugin in `.claude-plugin/`, the Codex plugin in `.codex-plugin/`,
the Gemini extension `gemini-extension.json`, the opencode `opencode.json`, and
the pi extension `pi-extension/` + root `package.json`).

The always-on rule has **one** source: `skills/opys/agent-rule.md`.
There are deliberately no committed per-editor copies — `opys agent-rules --tool
<editor>` (`commands/agent_rules.rs`) generates them on demand from that file,
which is embedded in the binary via `templates::AGENT_RULE` (`include_str!`) and
also referenced by the Gemini/opencode manifests and read by the pi extension.
Edit the rule in one place; everything else derives from it.
