# Work item file format (normative)

A **work item** is the ephemeral companion to a feature. Features document
*what the product does* and live forever; a work item captures *the in-flight
effort to change it* — a task checklist, a progress log, and links to branches,
commits, and PRs. It is a file so agents can `grep`/`find` it with no external
API, and it is **deleted on completion** — a work item adds no permanent
documentation. Anything worth keeping after the change ships belongs in a
feature file (see `references/format.md`).

The subsystem is opt-in: it exists only when `work-items/_config.toml` is
present (created by `opys work-item init`).

## Layout

Work items live under the same base directory as features (default `docs/opys/`,
configurable with `--dir` / `OPYS_DIR`):

```
docs/opys/
  work-items/
    _config.toml        # work-item configuration (see below)
    WI-0001.md
    ...
    INDEX.md            # generated — never hand-edit
  features/             # the permanent inventory (references/format.md)
  views/                # generated — work-item views live in wi-by-feature/, wi-status/
```

Work-item IDs are `WI-NNNN` — a fixed prefix, distinct from the feature prefix,
so an ID is unambiguous about which subsystem it names. Work items are deleted
on close, so they never accumulate; there is no sharding guidance.

## Configuration: `docs/opys/work-items/_config.toml`

```toml
pad = 4                             # zero-padding width (prefix is fixed: WI)
extra_statuses = []                 # beyond the four core statuses
required_sections = ["Tasks", "Progress"]   # body sections verify enforces

[fields.pr]                         # per-project custom frontmatter fields
type = "string"                     # string | list | bool | int
required = false
description = "Primary pull-request URL for this effort"
```

Custom fields work exactly as for features: a field used in any work-item file
must be declared here or verify fails. There is no `prefix` key — the work-item
prefix is fixed at `WI`.

## A complete work item file

````markdown
---
id: WI-0042
status: in-progress
tags: [osc]
references:
  FEAT-0421: Tab title follows OSC 0/2 sequence
---

# Make tab title survive profile switch

## Tasks
- [x] Reproduce the reset on profile switch
- [x] Lift title state out of the per-profile struct — see FEAT-0421
- [ ] Cover the switch path with a test

## Progress
- 2026-06-13 — root-caused: title held on `Profile`, dropped on swap; branch `wi-0042`
- 2026-06-14 — lifted title to `Window`; commit `a1b9f3c`; opened PR #318 (draft)

## Notes
Scratch space — hypotheses, dead ends. Free-form and unverified; deleted on
close, so nothing a future reader needs lives only here.
````

## Frontmatter

Frontmatter is **standard YAML** between `---` fences — the same parser and
canonical-serialization rules as feature files (core fields first, then
remaining keys alphabetically; the same colon-followed-by-space quoting
footgun). Reserved keys:

| Field | Required | Rules |
|---|---|---|
| `id` | yes | `WI-NNNN`; must match filename; never reused (see below) |
| `status` | yes | `todo` \| `in-progress` \| `blocked` \| `done` (+ configured extras) |
| `references` | yes | ID→title map; **must include ≥1 `FEAT-` id resolving to a live feature** |
| `tags` | no | if present, non-empty list, lowercase kebab-case |
| `blocked_reason` | iff blocked | one line on what the item is waiting on |
| custom | per config | validated against `[fields.*]` declarations |

There is deliberately no `assignee`, `priority`, `sprint`, `started`, or `due`
field — work items track effort *content*, not scheduling. This is the same
not-a-task-board stance features take.

### The `references` map

Features and work items share one uniform `references` field: a YAML mapping
keyed by ID (`FEAT-NNNN` / `WI-NNNN`), value = the referenced doc's title.
Prefixes are self-describing, so a single field captures links in both
directions — a work item references the feature(s) it changes; the feature
references its work items.

opys keeps this map **bidirectional and title-fresh automatically** on every
mutating command (and `sync-views`): if either side references the other, the
reverse link is added; every value is refreshed to the referenced doc's current
title. You do not hand-maintain it. Entries are always sorted by item number.
Bare `FEAT-`/`WI-` mentions in body prose are also rewritten into markdown
links (`[FEAT-0421 — …](../features/FEAT-0421.md)`), skipping code spans.

A work item must reference at least one `FEAT-` id that resolves to a live
feature — that link is the whole point of the subsystem. verify enforces it.

## Required body sections

`required_sections` (default `["Tasks", "Progress"]`) are enforced at write time
(`opys work-item new` scaffolds them) and re-checked by verify:

- **`# Title`** — one `# ` heading, non-empty.
- **`## Tasks`** — a GitHub-style checklist (`- [ ]` / `- [x]`). Unlike a feature
  test plan, a task is *work to do*, not a behavioral case, and carries **no**
  test-reference requirement. Tasks are mutable and disposable; they vanish on
  close.
- **`## Progress`** — a dated log where branch names, commit SHAs, and PR links
  accrete. verify only requires the heading to exist.

## Status semantics

- `todo` — created, not started (the default for `new`).
- `in-progress` — actively being worked.
- `blocked` — stalled on something external; `blocked_reason` required.
- `done` — terminal, and **only reached via `close`** (which deletes the file).
  `set-status … done` is rejected; there is no "done" file resting on disk.

Status changes go through `opys work-item set-status`, never hand edits.

## The close lifecycle

`opys work-item close <id>` is the only terminal operation. It:

1. refuses unless every `## Tasks` item is checked (override with `--force`);
2. **deletes** `WI-NNNN.md`;
3. **strikes through** the work item's title in every doc that references it
   (`WI-0042: ~~Make tab title survive profile switch~~`).

The struck reference is the tombstone. It keeps the link visible in the
feature, marks the work as done, and **reserves the ID forever** — ID
allocation scans every `references` map for `WI-` keys, struck or not, so a
number is never reused. There is no archive directory and no separate ledger;
the struck reference is the entire record.

> Fold anything durable back into the feature **before** closing — a new
> test-plan case, a status change to `implemented`, spec prose. The work item is
> deleted; whatever a future reader needs must already be in the feature.

`opys work-item cleanup` strips struck-through references from all docs when you
want to declutter; afterward the closed work items have no record except git
history, and their IDs may be reused.

## verify

A reference must resolve to an existing feature or work item **unless it is a
struck-through tombstone**, which is always accepted. Title drift and a missing
reverse link are *not* errors — they are auto-fixed by sync, not enforced. The
only reference failures are a non-struck id that resolves to nothing, and a work
item that references no live feature.

## What never goes in a work item

Permanent specifications, behavioral contracts, the enduring test-plan
enumeration, manual-verification procedures, scope decisions — all of those are
feature material and survive; a work item is deleted, so anything recorded only
here is lost on close. Conversely, a feature file never holds implementation
logs, task checklists, or branch/PR links — that is work-item material. The
dividing line: **would this still be true and useful after the change ships?**
Yes → feature. No → work item.
