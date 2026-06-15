---
name: opys
description: Set up and operate a file-based feature inventory ("file-based JIRA") — one markdown file per feature with YAML frontmatter, stable IDs, tags, test plans, manual-verification runbooks, and a verify gate for CI. It also tracks work items — ephemeral, per-change companion files (tasks, a progress log, branch/PR links) that link to features and are deleted on completion. Use this skill whenever the user wants to track implemented features, requirements coverage, feature parity with another product, a traceability matrix between features and tests, in-flight implementation work, or asks how to share a large feature list between themselves and LLM agents. Also use it when working inside a project that already has a docs/opys/features/ directory with _config.toml — for creating features, changing status, updating test plans, creating and closing work items, running verify, or generating views, reports, and manual-test runbooks.
---

# opys — feature inventory & work items

A version-controlled inventory of what a product does (features) and the
in-flight work changing it (work items): one markdown file per item, managed by
the `opys` CLI, verified in CI. Features are the *permanent* inventory and their
test coverage; work items are *ephemeral* per-change companions. Deliberately
not a task board (no sprints, assignees, priorities).

Read `references/format.md` before authoring or editing feature files, and
`references/work-items.md` before creating or editing work items — they are the
normative file-format specs and design rationale. This file covers operation.

> **Feature vs work item.** A *feature* is a permanent record of what the
> product does — spec, test plan, manual verification — and is never deleted
> (retired IDs are logged, not reused). A *work item* is a throwaway record of
> one in-flight change — tasks, a progress log, branch/PR links — that links to
> the feature(s) it touches and is **deleted on completion**. Put durable
> knowledge in features; put "what I'm doing right now" in work items. If
> unsure, ask: *does this stay true after the change ships?* Yes → feature;
> no → work item.

## Core principles (derive answers from these)

1. One file per feature; metadata (frontmatter) and spec prose live together.
2. Taxonomy never lives in filesystem layout — classification is multi-valued
   `tags`; all groupings are generated views.
3. Stable IDs are the contract. Tests, commits, and specs reference
   `FEAT-NNNN` (and `WI-NNNN` for work items). IDs are never reused or
   renumbered, even after deletion.
4. Intent is stored; derived state is generated. Test pass/fail, dates, and
   completion claims never go into feature files.
5. Writes go through the CLI (prevents parallel-agent collisions, enforces
   invariants at write time); reads are grep + targeted file reads.
6. Lazy growth: frontmatter + a title is a complete feature file. Prose, test
   plans, and manual procedures are added only where earned.

## The CLI

`opys` is a single self-contained Rust binary (no runtime dependencies).
Install it with `cargo install opys`, or build from source with `cargo build
--release` and drop the binary on `PATH`. Run it from the project root, or pass
`--root <dir>`. Because it is a published crate, project CI can install it in
one step.

The inventory lives under a base directory (default `docs/opys/`, set with
`--dir` or `OPYS_DIR`): `docs/opys/features/` (config + feature files +
`INDEX.md`), `docs/opys/work-items/` (optional), `docs/opys/views/`,
`docs/opys/runbooks/`. Mutating commands regenerate `INDEX.md` and `views/`
automatically (pass `--no-sync` to skip).

| Command | Purpose |
|---|---|
| `init` | bootstrap `docs/opys/features/_config.toml`, print CLAUDE.md snippet |
| `new --title T --tags a,b [--status S] [--reason R] [--field k=v]` | create file with next ID (auto-syncs) |
| `import FILE.jsonl` | bulk-create features from JSONL (sequential IDs, one sync, transactional) — for migrations |
| `show ID` / `list [--tag T] [--status S] [--field k=v]… [--format table\|ids\|paths]` | retrieval; `--field` filters by any custom field (repeatable, ANDed) |
| `set-status ID S [--reason R]` | guarded transitions (wontfix needs reason; implemented needs a checked test item) |
| `tag ID --add a,b --remove c` | tag maintenance |
| `block ID --by BLOCKER` / `unblock ID --by BLOCKER` | record/remove a blocker link (`blocked_by`/`blocks`, bidirectional); blocking a work item auto-sets `blocked` |
| `retire ID --reason R` | delete file, log ID to `_retired.txt` so it is never reallocated |
| `verify` | full integrity check (features + work items); nonzero exit on problems — wire into CI |
| `sync-views` | reconcile references, linkify prose, regenerate `INDEX.md` + `views/` (for hand edits) |
| `report` | status counts, coverage gaps, and (opt-in) parity % |
| `manual-runbook [--out docs/opys/runbooks/X.md]` | aggregate all manual items into an executable checklist, grouped by Setup, uncovered ones flagged ⚠ |
| `schema --kind config\|frontmatter` | emit a JSON Schema for editor/CI validation |

### Work-item commands

Work items (`references/work-items.md`) are the ephemeral companions to
features. Enable them with `opys work-item init`; they use the fixed `WI-NNNN`
prefix and live in `docs/opys/work-items/`. (Alias: `opys wi …`.)

| Command | Purpose |
|---|---|
| `work-item init` | scaffold `docs/opys/work-items/_config.toml` |
| `work-item new --title T --features F1,F2 [--tags a,b] [--status S] [--field k=v]` | create file with next `WI` ID; linked features must exist (auto-syncs) |
| `work-item show ID` / `list [--feature F] [--status S] [--field k=v]… [--format …]` | retrieval; `--field` filters by any custom field |
| `work-item set-status ID S [--reason R]` | guarded transition (`todo`/`in-progress`/`blocked`; `done` is reached only via `close`) |
| `work-item tag ID --add a,b --remove c` | tag maintenance (work-item tags are optional) |
| `work-item close ID [--force]` | finish: delete the file and strike its title through in every referencing doc (the struck reference reserves the ID) |
| `work-item cleanup` | strip struck-through (completed) work-item references from all docs |

## Workflow: bootstrapping a project

1. Run `opys init`, then edit `docs/opys/features/_config.toml`: set
   `test_search_paths`, and declare any project-specific frontmatter fields
   under `[fields.<name>]` (type, required, description). Unknown fields in
   feature files fail verify until declared — this keeps the schema honest.
   For parity projects set `parity = true`. To validate that test references
   point at real tests, set `test_reference_check = "extract"` plus a
   `test_name_pattern` regex; otherwise the default `"grep"` substring check
   applies. `opys schema --kind config` and `--kind frontmatter` emit JSON
   Schemas you can wire into editors (Even Better TOML) or CI to stop
   hallucinated fields.
2. Add the printed snippet to the project's CLAUDE.md.
3. Add `opys verify` (and optionally a `sync-views` freshness diff) to CI.
4. If migrating an existing feature list: at small scale, convert each entry
   with `new` (status `planned`, best-effort tags). At scale (hundreds+), do
   **not** loop `new` — emit a JSONL file and run `opys import` once, or write
   canonical `FEAT-NNNN.md` files directly then `opys sync-views` + `opys
   verify` (see "Bulk creation and migration" in `references/format.md`). Then
   review in batches per tag using generated views; archive the source
   document. Do not write spec prose during migration unless the source
   contains real behavioral detail.

## Workflow: implementing a feature (for coding agents)

1. Read `docs/opys/features/INDEX.md`, locate the feature, read its file fully.
2. Implement. Add tests.
3. In the test plan, check the covered items and append backticked test
   references — `module::test_name`, or `path/to/file::test_name` when the
   project uses `extract` mode. A case may be covered by several tests (list
   several refs), and one test may cover several cases. If the enumeration of
   cases looks incomplete versus the spec prose, raise it — do not silently
   implement only the listed cases.
4. `opys set-status ID implemented` (the CLI rejects this if no checked
   test item exists), then `opys verify`.

## Workflow: doing a piece of work (for coding agents)

A work item is your scratchpad for one change — a file, so it survives context
resets and is greppable, deleted when you finish.

1. Identify the feature(s) you will change; read their files.
2. `opys work-item new --title "…" --features FEAT-0001`. This scaffolds
   `## Tasks` / `## Progress` and links the feature(s); the CLI rejects a link to
   a feature that doesn't exist, and auto-adds the reverse link on the feature.
3. As you work, edit `## Tasks` (check items off) and append dated `## Progress`
   lines with branch/commit/PR — normal file edits. Don't hand-maintain the
   `references` map or linkify prose; `opys` does both on each write.
4. Fold anything durable back into the **feature**, not the work item: check the
   covered test-plan items and add refs, `opys set-status … implemented`, write
   spec prose. The feature is what survives.
5. `opys work-item close ID` — deletes the file and strikes its reference through
   in the feature as a tombstone. Do this only after step 4.
6. `opys verify`.

## Workflow: authoring features (interview style)

When drafting a feature file with a user, ask for edge cases — they become
test-plan items. Then ask which behaviors warrant a human eye on a real
build, and record a manual-verification item with Setup / numbered Steps /
Expect while the details are fresh. Manual verification is *not* reserved for
the unautomatable: a manual item may re-check behavior that automated tests
also cover (a friendlier, end-to-end sanity pass). To mark it as also
automated, add backticked test refs on the item's line; items with no refs
have no automated coverage and are flagged ⚠ and prioritized in the runbook
and counted in `report`.

## Retrieval discipline

Never bulk-read `docs/opys/features/`. The path is: `INDEX.md` (the one
whole-inventory file, deliberately small) → `rg` by tag/status or `list` →
read the 2–5 relevant files. Generated `views/` files are read-only
conveniences; regenerate with `sync-views`, never edit. Work items follow the
same discipline: `docs/opys/work-items/INDEX.md` → `rg`/`work-item list
--feature FEAT-0001` → the relevant `WI-NNNN.md` files.

## Release testing

`manual-runbook --out docs/opys/runbooks/release-X.md` produces the checklist,
grouped by Setup line so environments are reconfigured once, not per item;
items without automated coverage are flagged ⚠ so you prioritize them. The
executed, annotated runbook is committed — that file, not the feature files,
is where manual results live.
