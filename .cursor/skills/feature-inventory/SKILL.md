---
name: feature-inventory
description: Set up and operate a file-based feature inventory ("file-based JIRA") — one markdown file per feature with YAML frontmatter, stable IDs, tags, test plans, manual-verification runbooks, and a verify gate for CI. Use this skill whenever the user wants to track implemented features, requirements coverage, feature parity with another product, a traceability matrix between features and tests, or asks how to share a large feature list between themselves and LLM agents. Also use it when working inside a project that already has a docs/features/ directory with _config.toml — for creating features, changing status, updating test plans, running verify, or generating views, reports, and manual-test runbooks.
---
# Feature Inventory

A version-controlled inventory of what a product does: one markdown file per
feature, managed by the `opys` CLI, verified in CI. It tracks the *permanent
inventory* of features and their test coverage — deliberately not a task board
(no sprints, assignees, priorities).

Read `references/format.md` before authoring or editing feature files — it is
the normative file-format spec and design rationale. This file covers
operation.

## Core principles (derive answers from these)

1. One file per feature; metadata (frontmatter) and spec prose live together.
2. Taxonomy never lives in filesystem layout — classification is multi-valued
   `tags`; all groupings are generated views.
3. Stable IDs are the contract. Tests, commits, and specs reference
   `PREFIX-NNNN`. IDs are never reused or renumbered, even after deletion.
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

The inventory lives under a base directory (default `docs/`, set with `--dir`
or `OPYS_DIR`): `docs/features/` (config + feature files + `INDEX.md`),
`docs/views/`, `docs/runbooks/`. Mutating commands regenerate `INDEX.md` and
`views/` automatically (pass `--no-sync` to skip).

| Command | Purpose |
|---|---|
| `init` | bootstrap `docs/features/_config.toml`, print CLAUDE.md snippet |
| `new --title T --tags a,b [--status S] [--field k=v]` | create file with next ID (auto-syncs) |
| `show ID` / `list [--tag T] [--status S] [--format table\|ids\|paths]` | retrieval |
| `set-status ID S [--reason R]` | guarded transitions (wontfix needs reason; implemented needs a checked test item) |
| `tag ID --add a,b --remove c` | tag maintenance |
| `retire ID --reason R` | delete file, log ID to `_retired.txt` so it is never reallocated |
| `verify` | full integrity check; nonzero exit on problems — wire into CI |
| `sync-views` | regenerate `docs/features/INDEX.md` + `views/by-tag/`, `views/status/` (for hand edits) |
| `report` | status counts, coverage gaps, and (opt-in) parity % |
| `manual-runbook [--out docs/runbooks/X.md]` | aggregate all manual items into an executable checklist, grouped by Setup, uncovered ones flagged ⚠ |
| `schema --kind config\|frontmatter` | emit a JSON Schema for editor/CI validation |

## Workflow: bootstrapping a project

1. Run `opys init`, then edit `docs/features/_config.toml`: set `prefix`,
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
4. If migrating an existing feature list: convert each entry with `new`
   (status `planned`, best-effort tags), then review in batches per tag using
   generated views; archive the source document. Do not write spec prose
   during migration unless the source contains real behavioral detail.

## Workflow: implementing a feature (for coding agents)

1. Read `docs/features/INDEX.md`, locate the feature, read its file fully.
2. Implement. Add tests.
3. In the test plan, check the covered items and append backticked test
   references — `module::test_name`, or `path/to/file::test_name` when the
   project uses `extract` mode. A case may be covered by several tests (list
   several refs), and one test may cover several cases. If the enumeration of
   cases looks incomplete versus the spec prose, raise it — do not silently
   implement only the listed cases.
4. `opys set-status ID implemented` (the CLI rejects this if no checked
   test item exists), then `opys verify`.

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

Never bulk-read `docs/features/`. The path is: `INDEX.md` (the one
whole-inventory file, deliberately small) → `rg` by tag/status or `list` →
read the 2–5 relevant files. Generated `views/` files are read-only
conveniences; regenerate with `sync-views`, never edit.

## Release testing

`manual-runbook --out docs/runbooks/release-X.md` produces the checklist,
grouped by Setup line so environments are reconfigured once, not per item;
items without automated coverage are flagged ⚠ so you prioritize them. The
executed, annotated runbook is committed — that file, not the feature files,
is where manual results live.
