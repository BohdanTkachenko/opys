---
name: opys
description: Set up and operate a file-based inventory of typed markdown documents ("file-based JIRA") — one markdown file per document with YAML frontmatter, stable IDs, tags, and configurable types/statuses/fields/sections/rules in one opys.toml, with test plans and a verify gate for CI. The default config ships a permanent feature type plus ephemeral task/bug/chore types (deleted on close); projects can add their own (epic, adr, risk, …). Use this skill whenever the user wants to track implemented features, requirements coverage, feature parity with another product, a traceability matrix between features and tests, in-flight implementation work, or asks how to share a large list between themselves and LLM agents. Also use it when working inside a project that already has a opys.toml — for creating documents (opys new --type), changing status, updating test plans, closing work, running verify, or viewing the index and stats.
---

# opys — typed-document inventory

A version-controlled inventory of typed markdown documents — what a product does
(permanent `feature` documents and their test coverage) and the in-flight work
changing it (ephemeral `task`/`bug`/`chore` documents) — one markdown file per
item, managed by the `opys` CLI, verified in CI. Document types are configured in
`opys.toml`; projects add their own. Deliberately not a task board (no
sprints, assignees, priorities).

Read `references/format.md` before authoring or editing documents or the
`opys.toml` config — it is the normative file-format spec and design rationale.
This file covers operation.

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
   `tags`; all groupings are live queries (`opys list`).
3. Stable IDs are the contract. Tests, commits, and specs reference
   `FEAT-NNNN` (and `TASK-`/`BUG-`/`CHORE-NNNN` for work items). IDs are never
   reused or renumbered, even after deletion.
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

`opys.toml` lives at the **project root** (opys finds it by searching upward from
the cwd, like git/Cargo). It declares a `base` directory (default `opys/`,
relative to the root) holding the document files, flat at `opys/` by default (the
path is rendered from a configurable `[layout]` template). Mutating commands
auto-sync — reconcile, linkify, and relocate docs to their layout path (pass
`--no-sync` to skip).

The document **types** are configured in `opys.toml` (`opys config init` writes
the opinionated default; `opys config validate` checks it). The default ships a
permanent `feature` type plus ephemeral `task`/`bug`/`chore` types
(`TASK-`/`BUG-`/`CHORE-NNNN`, deleted on `close`); add a `[types.<name>]` block
for your own (prefix, statuses, fields, sections, rules). A document's type is
its ID prefix.

| Command | Purpose |
|---|---|
| `init` | bootstrap `opys.toml` + `opys/`, print CLAUDE.md snippet |
| `config init` / `config validate` | generate / check the universal `opys.toml` |
| `new --type T --title … [--tags a,b] [--status S] [--features F1,F2] [--reason R] [--field k=v]` | create a doc of type `T` with the next ID (`--type` defaults to `feature`; auto-syncs) |
| `import --type T FILE.jsonl` | bulk-create docs of type `T` from JSONL (sequential IDs, one sync, transactional) |
| `show ID` / `list [--type T] [--tag T] [--status S] [--field k=v]… [--format table\|ids\|paths]` | retrieval; `--field` filters by any custom field (repeatable, ANDed) |
| `set-status ID S [--reason R]` | guarded transition against the type's statuses + rules; a terminal status is reached only via `close` |
| `tag ID --add a,b --remove c` | tag maintenance |
| `block ID --by BLOCKER` / `unblock ID --by BLOCKER` | record/remove a blocker link (`blocked_by`/`blocks`); blocking auto-sets `blocked` when the type has it |
| `retire ID --reason R` | delete file, log ID to `_retired.txt` so it is never reallocated |
| `close ID [--force]` / `cleanup` | finish a doc whose type has a terminal status (strike its refs everywhere); strip struck refs |
| `verify` | full integrity check; nonzero exit on problems — wire into CI |
| `sync` | reconcile references, linkify prose, relocate docs to their layout path |
| `stats` | per-type status counts + percentages, and coverage gaps |
| `agent-rules --tool <editor>` | generate a rules-based editor's instruction file from the canonical rule |

## Workflow: bootstrapping a project

1. Run `opys init`, then edit `opys.toml`: declare your document
   `[types.<name>]` (prefix, statuses, `[fields.*]`, `[[sections]]`, and
   `[[rules]]`). Unknown frontmatter fields fail verify until declared on the
   type — this keeps the schema honest. To validate that a section's references
   point at something real (e.g. a test plan's `mod::name` refs, or a
   `` `file` — `symbol` `` code-pointer line), attach a
   `[[types.<name>.sections.checks]]` — a `pattern` parsing each line into named
   groups plus a `file` and/or `must_match` resolving them against `roots`. The
   default config ships one on the feature `Test plan`. `opys config validate`
   checks the config is well-formed.
2. Add the printed snippet to the project's CLAUDE.md.
3. Add `opys verify` (and optionally a `sync` freshness diff) to CI.
4. If migrating an existing feature list: at small scale, convert each entry
   with `new` (status `planned`, best-effort tags). At scale (hundreds+), do
   **not** loop `new` — emit a JSONL file and run `opys import` once, or write
   canonical `FEAT-NNNN.md` files directly then `opys sync` + `opys
   verify` (see "Bulk creation and migration" in `references/format.md`). Then
   review in batches per tag using `opys list`; archive the source
   document. Do not write spec prose during migration unless the source
   contains real behavioral detail.

## Workflow: implementing a feature (for coding agents)

1. Locate the feature (`opys list` / `rg` by tag or status), read its file fully.
2. Implement. Add tests.
3. In the test plan, check the covered items and append backticked test
   references in the shape the section's check expects — by default
   `module::test_name` (or `path/to/file::test_name`). A case may be covered by
   several tests (list several refs), and one test may cover several cases. If the
   enumeration of cases looks incomplete versus the spec prose, raise it — do not
   silently implement only the listed cases.
4. `opys set-status ID implemented` (the CLI rejects this if no checked
   test item exists), then `opys verify`.

## Workflow: doing a piece of work (for coding agents)

A work item is your scratchpad for one change — a file, so it survives context
resets and is greppable, deleted when you finish.

1. Identify the feature(s) you will change; read their files.
2. `opys new --type bug --title "…" --features FEAT-0001` (or `--type task`/
   `chore`). This scaffolds the type's required sections (`## Tasks` / `## Progress`,
   plus `## Reproduction` for a bug) and links the feature(s); the CLI rejects a
   link to a feature that doesn't exist, and auto-adds the reverse link.
3. As you work, edit `## Tasks` (check items off) and append dated `## Progress`
   lines with branch/commit/PR — normal file edits. Don't hand-maintain the
   `references` map or linkify prose; `opys` does both on each write.
4. Fold anything durable back into the **feature**, not the work item: check the
   covered test-plan items and add refs, `opys set-status … implemented`, write
   spec prose. The feature is what survives.
5. `opys close ID` — deletes the file and strikes its reference through in the
   feature as a tombstone. Do this only after step 4.
6. `opys verify`.

## Workflow: authoring features (interview style)

When drafting a feature file with a user, ask for edge cases — they become
test-plan items. Then ask which behaviors warrant a human eye on a real
build, and record a manual-verification item with Setup / numbered Steps /
Expect while the details are fresh. Manual verification is *not* reserved for
the unautomatable: a manual item may re-check behavior that automated tests
also cover (a friendlier, end-to-end sanity pass). To mark it as also
automated, add backticked test refs on the item's line; items with no refs
have no automated coverage and are counted in `opys stats`.

## Retrieval discipline

Never bulk-read `opys/`. The path is: `rg` by tag/status or
`opys list [--type T]` → read the 2–5 relevant files. There is no generated index
— slice the inventory live with `opys list`, not pre-generated files.
