# Agent Instructions

This file provides guidance to AI coding assistants (Claude Code and any other AGENTS.md-aware tool) when working with code in this repository.

## What this is

`opys` is a Rust CLI that manages a **file-based inventory of typed markdown
documents**: one markdown file per document, each with `---`-fenced YAML
frontmatter (a stable `PREFIX-NNNN` id, status, tags, relation maps) plus a
markdown body. All writes go through the CLI so invariants hold at write time and
parallel agents don't collide; reads are plain `grep` + targeted file reads.
`verify` is the CI gate. The inventory base dir defaults to `opys/`. It is
deliberately *not* a task board — no sprints, assignees, or priorities.

Everything is driven by **one config, `opys.toml`** (parsed into `ProjectConfig`,
`src/project_config.rs`), which lives at the **project root** — `Project::open`
finds it by searching upward from the cwd (`find_root`), and it declares the
inventory `base` (default `opys/`, relative to the root). The config
declares document **types**, each with an id `prefix`, an optional `dir` and
per-status `status_dirs` (the `{type}`/`{status}` segments of the configurable
`[layout]` path template — both empty by default, so docs live flat at `base`),
its own `statuses`
(plus `default_status` / `terminal_statuses`), `[fields.*]` (custom frontmatter
fields, with optional regex `pattern`), and required `sections` (each a
code-backed *kind*: prose/log/checklist/structured — the `structured` kind's
content shape is config-driven via a `structure` (an `mdprism` schema, the
`mdprism` workspace crate) — with optional config-driven
`checks`), plus a list of
conditional `[[rules]]` (`when {type?, status?}` + one assertion). **A document's
type is its id prefix.** There is no hardcoded type set: the default config ships
a permanent `feature` type plus ephemeral `task`/`bug`/`chore` types (deleted on
`close`), but a project can add `epic`, `adr`, `risk`, … and the whole tool
(create, verify, index) works for them. The engine that runs the rules is
`src/rules.rs` (`rules::evaluate`).

The tool ships alongside the tool-agnostic `opys` skill in `skills/opys/`. The
normative spec lives at `skills/opys/references/format.md` — consult it before
changing parsing, serialization, or `verify` semantics, and keep code ↔
format.md in sync. The README explains how users copy that one folder into their
tool's skills directory (`.claude/skills/`, `.cursor/skills/`, `.agents/skills/`).

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
  `maybe_sync()` (the auto-sync hook: reconcile + linkify + relocate).
- `src/project_config.rs` — `ProjectConfig` (the parsed `opys.toml`): the `types`
  map of `DocType` (prefix, dir, status_dirs, statuses, fields, sections, the
  `requires_link` shorthand), the `[layout]` path template, and the `[[rules]]`
  list, plus `type_name_for_id`, `doc_relpath` (renders a doc's canonical path),
  and config self-validation (`validate`). The sole config.
- `src/rules.rs` — `rules::evaluate(prj, type, status, fm, body, doc_ids)`: runs
  the applicable `[[rules]]` (plus the type-level `requires_link` shorthand) and
  returns one problem per failed assertion. Called at every write point and by
  `verify`.
- `src/project.rs` — `Project` ties the on-disk layout to `pcfg`. `Project::open`
  requires `<base>/opys.toml`. Owns generic discovery (`load_docs`: scan the base
  recursively for ID-named files, parse into `Doc`), the canonical-path helper
  (`doc_path`) and `save_doc` (write + relocate to the canonical layout path), ID
  allocation (`max_doc_id`/`next_id_for` over one global sequence), the retired-ID
  ledger (`<base>/_retired.txt`), `find`/`find_mut`, and the shared regexes
  (`id_format_re`, `KEBAB_RE`, `parse_field`).
- `src/doc.rs` / `src/frontmatter.rs` / `src/body.rs` — the parse layer. `Doc` is
  the single document struct (`{path, frontmatter, body, title}`; type derived
  from the id prefix). `frontmatter` parses YAML with `serde_norway` and
  re-serializes canonically; `body` extracts the title, sections
  (`section(body, heading)`), and checkbox items (`checklist_items(body, heading)`).
- `src/refs.rs` — the uniform relation maps (`references`/`blocked_by`/`blocks`),
  ID→title: parse/serialize (sorted by item number), strikethrough tombstone
  helpers, `id_number`.
- `src/links.rs` — the auto-sync engine: `reconcile`/`reconcile_blockers`
  (bidirectional, title-fresh relation maps between live docs) and `linkify`
  (bare `PREFIX-NNNN` mentions in prose → markdown links, skipping code; the
  prefix regex is built by `ref_re` from the live type prefixes). Driven by
  `commands/sync.rs`, which `maybe_sync` calls.
- `src/config.rs` — just the shared `FieldSpec` / `FieldType` / `TestRefCheck`
  the engine config reuses.

### Invariants enforced on disk (the point of the tool)

All status/section/link guards are *config*, enforced by one engine
(`rules::evaluate`) at every write point and re-checked by `verify`.

- **IDs**: each type has a `prefix` (validated `^[A-Z][A-Z0-9]*$`, unique across
  types); ids are drawn from a *single global, monotonically increasing
  sequence* — never reused, never duplicated across prefixes. `max_doc_id` takes
  the max over every live doc, the retired ledger (`<base>/_retired.txt`), *and*
  every relation map (`refs::all_relation_ids`, struck or not), so a closed doc's
  tombstone still reserves its number; `next_id_for(prefix, …)` is one past it.
  `retire` appends to the (sorted) ledger; `verify` rejects reuse *and* any two
  live docs sharing a number (`check_unique_numbers`).
- **References** (`references` map): auto-reconciled on every write
  (`links::reconcile`) — bidirectional between live docs, titles refreshed, sorted
  by number. A closed doc leaves a struck-through (`~~title~~`) tombstone.
  `verify` fails on a non-struck id that resolves to nothing, or a type whose
  `requires_link` is unmet; drift / missing reverse links are auto-fixed, not
  gated. Bare ID mentions in body prose are linkified (`links::linkify`),
  skipping code spans/fences.
- **Blockers** (`blocked_by` / `blocks` maps): a directional relation on the same
  ID→title machinery. `opys block <id> --by <id>` / `unblock` write `blocked_by`
  on the blocked side and the inverse `blocks` on the blocker. Blocking a doc
  whose type has a `blocked` status auto-sets it (the link satisfies the
  blocked-reason rule); `unblock` reverts to `in-progress` when no blocker/reason
  remains. `refs::RELATION_FIELDS` drives close/cleanup/verify/id-reservation
  uniformly.
- **Status lifecycle**: each type declares its own `statuses`, `default_status`,
  and `terminal_statuses`. No FSM — any status → any status — except a terminal
  status is reached only via `close` (`new`/`set-status` reject it). The
  conditional guards (e.g. feature `wontfix`⇒`wontfix_reason`, `implemented`⇒a
  checked `## Test plan` item; any `blocked`⇒a reason or blocker link) are
  `[[rules]]`, enforced at write time and by `verify`. "Removed from the product"
  is just a status (e.g. `archived`), never a deletion.
- **Section checks** (`[[types.X.sections.checks]]`): a universal, config-driven
  validation attachable to any section. A `pattern` regex parses each line into
  named capture groups; an optional `file` group (a path that must exist under
  `roots`) and/or `must_match` regex (built from `${group}` substitutions, matched
  in that file or the corpus) assert the parsed reference resolves. `scope`
  (`all`/`checked`) selects the lines. The default config reproduces the old
  test-plan grep with one such check on the `Test plan` `checklist`; the engine is
  `run_check` in `commands/verify.rs`. (There is no longer a `test-plan` kind or a
  `[tests]` block.)
- **Sections**: a type's `sections` each declare a `kind` (prose/log/checklist/
  structured), optional `checks`, and `required`. `verify` checks a required
  section is present, runs each section's `checks`, and validates a `structured`
  section's content against its `structure` (an `mdprism` schema) via
  `mdprism::Schema::validate`; `new` scaffolds the required ones (a `structured`
  section from `mdprism::Schema::scaffold`).
- **Frontmatter is closed**: only the reserved keys (`id`/`status`/`tags` +
  `references`/`blocked_by`/`blocks`) plus the doc type's declared `[fields.*]`
  are allowed; unknown keys fail `verify`. Declared fields are type-checked
  (`check_custom_fields`); a `type = "enum"` field constrains the value to its
  `values`, a `pattern` constrains a string, and `list --field key=value` filters
  on any of them.

### Auto-sync — no generated artifacts

opys generates no view files (no `INDEX.md`); slice the inventory live with
`opys list`. Mutating commands (`new`, `set-status`, `tag`, `retire`, `block`,
`close`, `cleanup`) call `maybe_sync` → `commands/sync::run` automatically unless
`--no-sync` is passed. That pass reconciles relations, linkifies prose, and
**relocates each document to its canonical layout path** (`save_doc` → `fs::rename`
when a status change or `[layout]` edit moved it, e.g. into `_archived/`); it
refuses to run if any document fails to parse (run `verify` first). `opys sync`
runs the same pass after hand edits (and deletes any stale `INDEX.md` left by
older versions).

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
