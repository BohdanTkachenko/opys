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
    TASK-0001.md
    BUG-0001.md
    ...
    INDEX.md            # generated — never hand-edit
  features/             # the permanent inventory (references/format.md)
  views/                # generated — work-item views: wi-by-feature/, wi-status/, wi-by-type/
```

### Work-item types

A work item is one of a fixed set of **types**, each with its own ID prefix and a
shared ephemeral lifecycle. The type is chosen at creation with
`opys work-item new --type <name>` (default `task`) and is **derived from the ID
prefix** — there is no `type:` frontmatter field, so the ID is the single source
of truth.

| Type | Prefix | For | Extra required section |
|---|---|---|---|
| `task` | `TASK-NNNN` | general implementation work (the default) | — |
| `bug` | `BUG-NNNN` | a defect in shipped behavior | `## Reproduction` |
| `chore` | `CHORE-NNNN` | maintenance/tooling, no behavior change | — |

All IDs — features and every work-item type — draw from **one global,
increasing sequence**, so a number never repeats across prefixes (you will see
e.g. `FEAT-0001`, `BUG-0002`, `TASK-0003`). The number alone is unique; the
prefix only names the type. Work items are deleted on close, so they never
accumulate; there is no sharding guidance. The types are hardcoded — projects do
not define their own.

## Configuration: `docs/opys/work-items/_config.toml`

```toml
pad = 4                             # zero-padding width for every type's ids
extra_statuses = []                 # beyond the four core statuses
required_sections = ["Tasks", "Progress"]   # shared baseline; types add their own

[fields.pr]                         # per-project custom frontmatter fields
type = "string"                     # string | list | bool | int | enum
required = false
description = "Primary pull-request URL for this effort"
```

`required_sections` is the **shared baseline** enforced for every type; a type
may require additional sections (e.g. `bug` adds `## Reproduction`). Custom
fields work exactly as for features: a field used in any work-item file must be
declared here or verify fails, an `enum` field constrains its value to a declared
`values` set, and `opys work-item list --field <key>=<value>` (and `--type`)
filter the listing. There is no `prefix`/`type` config — the type set is fixed.

## A complete work item file

````markdown
---
id: TASK-0042
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
| `id` | yes | `<TYPE>-NNNN` (TASK/BUG/CHORE); must match filename; never reused (see below) |
| `status` | yes | `todo` \| `in-progress` \| `blocked` \| `done` (+ configured extras) |
| `references` | yes | ID→title map; **must include ≥1 `FEAT-` id resolving to a live feature** |
| `blocked_by` / `blocks` | no | ID→title maps of the blocker relation; auto-maintained (see below) |
| `tags` | no | if present, non-empty list, lowercase kebab-case |
| `blocked_reason` | iff blocked | one line on what the item is waiting on, **unless** a `blocked_by` link supplies the reason |
| custom | per config | validated against `[fields.*]` declarations |

There is deliberately no `assignee`, `priority`, `sprint`, `started`, or `due`
field — work items track effort *content*, not scheduling. This is the same
not-a-task-board stance features take.

### The `references` map

Features and work items share one uniform `references` field: a YAML mapping
keyed by ID (`FEAT-NNNN` / `TASK-`/`BUG-`/`CHORE-NNNN`), value = the referenced
doc's title. Prefixes are self-describing, so a single field captures links in both
directions — a work item references the feature(s) it changes; the feature
references its work items.

opys keeps this map **bidirectional and title-fresh automatically** on every
mutating command (and `sync-views`): if either side references the other, the
reverse link is added; every value is refreshed to the referenced doc's current
title. You do not hand-maintain it. Entries are always sorted by item number.
Bare feature/work-item ID mentions in body prose are also rewritten into
markdown links (`[FEAT-0421 — …](../features/FEAT-0421.md)`), skipping code spans.

A work item must reference at least one `FEAT-` id that resolves to a live
feature — that link is the whole point of the subsystem. verify enforces it.

### Blockers

`blocked_by` / `blocks` are a second pair of auto-maintained ID→title maps,
distinct from `references`, recording a **directional** dependency. `opys block
<id> --by <blocker-id>` writes `blocked_by` on the blocked item and the inverse
`blocks` on the blocker (either may be a feature or work-item id); `opys unblock`
removes both sides. Blocking a work item auto-sets it to `blocked` and the link
satisfies the `blocked_reason` requirement; unblocking the last one reverts it to
`in-progress`. A blocker does **not** count as the required feature link. Like
`references`, blocker entries resolve or must be a struck tombstone, are struck
on `close`, reserve the closed id, and are stripped by `cleanup`.

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
- `blocked` — stalled on something external; requires `blocked_reason` *or* a
  blocker link. `opys block <id> --by <blocker>` records the blocker and
  auto-sets this status (the link is the reason); `opys unblock` reverts it to
  `in-progress` when no blocker or reason remains. See **Blockers** below.
- `done` — terminal, and **only reached via `close`** (which deletes the file).
  `set-status … done` is rejected; there is no "done" file resting on disk.

Status changes go through `opys work-item set-status`, never hand edits.

## The close lifecycle

`opys work-item close <id>` is the only terminal operation. It:

1. refuses unless every `## Tasks` item is checked (override with `--force`);
2. **deletes** `<TYPE>-NNNN.md`;
3. **strikes through** the work item's title in every doc that references it
   (`TASK-0042: ~~Make tab title survive profile switch~~`).

The struck reference is the tombstone. It keeps the link visible in the
feature, marks the work as done, and **reserves the number forever** — ID
allocation scans every relation map (all prefixes), struck or not, so a number
is never reused across the whole global sequence. There is no archive directory
and no separate ledger; the struck reference is the entire record.

> Fold anything durable back into the feature **before** closing — a new
> test-plan case, a status change to `implemented`, spec prose. The work item is
> deleted; whatever a future reader needs must already be in the feature.

`opys work-item cleanup` strips struck-through references from all docs when you
want to declutter; afterward the closed work items have no record except git
history, and their IDs may be reused.

## verify

A reference must resolve to an existing feature or work item **unless it is a
struck-through tombstone**, which is always accepted. The same rule applies to
`blocked_by` / `blocks` entries (which additionally may not list the item
itself). Title drift and a missing reverse link are *not* errors — they are
auto-fixed by sync, not enforced. The only reference failures are a non-struck id
that resolves to nothing, and a work item that references no live feature.

## What never goes in a work item

Permanent specifications, behavioral contracts, the enduring test-plan
enumeration, manual-verification procedures, scope decisions — all of those are
feature material and survive; a work item is deleted, so anything recorded only
here is lost on close. Conversely, a feature file never holds implementation
logs, task checklists, or branch/PR links — that is work-item material. The
dividing line: **would this still be true and useful after the change ships?**
Yes → feature. No → work item.
