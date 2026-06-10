---
name: feature-inventory
description: Set up and operate a file-based feature inventory ("file-based JIRA") — one markdown file per feature with YAML frontmatter, stable IDs, tags, test plans, manual-verification runbooks, and a verify gate for CI. Use this skill whenever the user wants to track implemented features, requirements coverage, feature parity with another product, a traceability matrix between features and tests, or asks how to share a large feature list between themselves and LLM agents. Also use it when working inside a project that already has a features/ directory with _config.toml — for creating features, changing status, updating test plans, running verify, or generating views, reports, and manual-test runbooks.
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

| Command | Purpose |
|---|---|
| `init` | bootstrap `features/_config.toml`, print CLAUDE.md snippet |
| `new --title T --tags a,b [--status S] [--field k=v]` | create file with next ID |
| `show ID` / `list [--tag T] [--status S] [--format table\|ids\|paths]` | retrieval |
| `set-status ID S [--reason R]` | guarded transitions (wontfix needs reason; implemented needs a checked test item) |
| `tag ID --add a,b --remove c` | tag maintenance |
| `retire ID --reason R` | delete file, log ID to `_retired.txt` so it is never reallocated |
| `verify` | full integrity check; nonzero exit on problems — wire into CI |
| `sync-views` | regenerate `features/INDEX.md` + `views/by-tag/`, `views/status/` |
| `report` | counts, parity % (with and without wontfix in denominator), coverage gaps |
| `manual-runbook [--out runbooks/X.md]` | aggregate all manual items into an executable checklist, grouped by Setup |

## Workflow: bootstrapping a project

1. Run `opys init`, then edit `features/_config.toml`: set `prefix`,
   `test_search_paths`, and declare any project-specific frontmatter fields
   under `[fields.<name>]` (type, required, description). Unknown fields in
   feature files fail verify until declared — this keeps the schema honest.
2. Add the printed snippet to the project's CLAUDE.md.
3. Add `opys verify` (and optionally a `sync-views` freshness diff) to CI.
4. If migrating an existing feature list: convert each entry with `new`
   (status `planned`, best-effort tags), then review in batches per tag using
   generated views; archive the source document. Do not write spec prose
   during migration unless the source contains real behavioral detail.

## Workflow: implementing a feature (for coding agents)

1. Read `features/INDEX.md`, locate the feature, read its file fully.
2. Implement. Add tests.
3. In the test plan, check the covered items and append backticked test
   references (`module::test_name`). If the enumeration of cases looks
   incomplete versus the spec prose, raise it — do not silently implement
   only the listed cases.
4. `opys set-status ID implemented` (the CLI rejects this if no checked
   test item exists), then `opys verify`.

## Workflow: authoring features (interview style)

When drafting a feature file with a user, ask for edge cases — they become
test-plan items. For anything automation cannot reach, ask "what are the
steps, and what does pass look like?" and record a manual-verification item
with Setup / numbered Steps / Expect immediately, while the details are fresh.
Every manual item records *why* it is manual; that is a standing challenge
for future automation.

## Retrieval discipline

Never bulk-read `features/`. The path is: `INDEX.md` (the one whole-inventory
file, deliberately small) → `rg` by tag/status or `list` → read the 2–5
relevant files. Generated `views/` files are read-only conveniences;
regenerate with `sync-views`, never edit.

## Release testing

`manual-runbook --out runbooks/release-X.md` produces the checklist, grouped
by Setup line so environments are reconfigured once, not per item. The
executed, annotated runbook is committed — that file, not the feature files,
is where manual results live.
