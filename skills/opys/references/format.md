# Document file format (normative)

## Layout

`opys.toml` lives at the **project root** — opys finds it by searching upward
from the current directory (like git or Cargo). It declares the inventory
`base` directory (default `opys/`, relative to the root), which holds the
documents:

```
<project root>/
  opys.toml             # the config (found by searching upward) — declares `base`
  opys/                 # the inventory base (config `base`, default opys)
    FEAT-0001.md        # documents live flat at the base by default
    FEAT-0002.md
    _archived/          # e.g. status_dirs = { archived = "_archived" }
      FEAT-0003.md
    _retired.txt        # append-only log of deleted IDs (never reused), sorted
```

Each document is one markdown file named after its ID (`FEAT-0001.md`,
`EPIC-0003.md`). **A document's type is its ID prefix.** Discovery scans the base
recursively and only treats ID-named files (`PREFIX-NNNN.md`) as documents, so
stray markdown (READMEs, notes) and `_`-prefixed files are ignored.

**Layout.** A document's path under `base` is rendered from the `[layout]`
`path` template (default `"{type}/{status}/{id}.md"`): `{type}` → the type's
`dir`, `{status}` → the type's `status_dirs[status]`, `{id}` → `PREFIX-NNNN`.
Both the `{type}` and `{status}` segments are empty by default, so documents
live flat at the base; empty segments collapse, so the order is free (e.g.
`"{status}/{type}/{id}.md"` groups by status first). Setting a doc's status (or
editing the layout) moves its file to the new canonical path on the next write —
relocated documents stay fully in the inventory. Directory structure must never
encode taxonomy — that is what tags and `opys list` are for.

## Configuration: `opys.toml`

One config declares every document **type** and the rules over them. `opys config
init` writes the opinionated default (a permanent `feature` type plus ephemeral
`task`/`bug`/`chore`); `opys config validate` checks it.

```toml
pad = 4                              # zero-padding width for the numeric id part

# [layout]                           # on-disk path template (relative to base)
# path = "{type}/{status}/{id}.md"   # default; {type}/{status} empty → flat

[types.feature]                      # one [types.<name>] block per document type
prefix = "FEAT"                      # ^[A-Z][A-Z0-9]*$, unique across types
# dir = "features"                   # the {type} layout segment; default empty (flat)
statuses = ["planned", "partial", "implemented", "wontfix", "archived"]
default_status = "planned"
terminal_statuses = []               # statuses reached only via `close` (which deletes)
status_dirs = { archived = "_archived" }   # per-status {status} segment; default empty
tags_required = true
# requires_link = { to = "feature", min = 1 }   # must reference ≥min docs of a type

[types.feature.fields.priority]      # custom frontmatter fields (the closed schema)
type = "enum"                        # string | list | bool | int | enum
values = ["low", "high"]             # an enum constrains the value
# pattern = '^JIRA-[0-9]+$'          # a string field may require a regex

[[types.feature.sections]]           # required/known body sections, by kind
heading = "Test plan"
kind = "checklist"                   # prose | log | checklist | manual
# required = true

[[types.feature.sections.checks]]    # universal content checks (see below)
pattern = '`(?P<ref>[^`]*::(?P<name>[^`]+))`'   # parse a line → named groups
roots = ["src", "tests"]             # file / corpus resolved against these (default ["."])
must_match = '${name}'               # regex; ${group} = the regex-escaped capture
scope = "checked"                    # "all" (every line) | "checked" (checked items)
message = "test reference `${ref}` not found"   # optional custom failure message

[[rules]]                            # conditional guards: a `when` + one assertion
when = { type = "feature", status = "implemented" }
require_checked_section = "Test plan"
```

A frontmatter field used on any document must be declared on its type or verify
fails — undeclared fields are how schema drift starts. An `enum` constrains its
value to the declared `values`; a string field may carry a `pattern`. The closed
assertion set for `[[rules]]` is `require_field`, `field_matches`,
`require_section`, `require_checked_section`, `require_link`, `require_any`.
`opys list --field <key>=<value>` filters by any custom field (see below).

### `[palette]` — TUI presentation (optional)

Purely cosmetic styling for the `opys tui` board; the core engine ignores it,
but `opys config validate` checks it so mistakes surface in CI. Each named entry
has `matchers` (`{status?, type?}`) and a `style`. A document matches an entry
when **any** matcher matches (a matcher matches when every field it sets equals
the document's; an empty matcher `{}` matches all). For a document, the styles
of all matching entries are merged field-wise in ascending **specificity**
(constrained-field count; ties by entry name), so more-specific rules win.

```toml
[palette.blocked]
matchers = [ { status = "blocked" } ]
[palette.blocked.style]
fg_color = "red"        # a name, #rrggbb / #rgb hex, or a 0–255 index
bg_color = "#111"
icon = "⏸"              # any string; overrides the default per-type glyph
bold = true
italic = false
strikethrough = false

[palette.bug]
matchers = [ { type = "bug" } ]
[palette.bug.style]
icon = "🐞"
```

Validation rejects a matcher whose `type` is not a defined type or whose
`status` is not a real status (of that type when both are given, else of any
type), an unparseable color, and an entry with no matchers. Where the palette
sets nothing, the TUI falls back to a default icon per type and color per status.

### `[tui]` — board columns (optional)

```toml
[tui]
columns = ["id", "title", "status", "priority", "updated"]
```

The list columns, left to right. Each is a built-in (`id`, `type`, `title`,
`status`, `tags`, `created`, `updated`) or the name of a custom frontmatter
field (shown as that field's value, blank where a document lacks it). Defaults to
`["id", "title", "status", "tags"]`. `config validate` rejects a column that is
neither a built-in nor a field declared on some type.

## A complete feature file

````markdown
---
id: FEAT-0421
status: implemented
tags: [osc, tabs, vte-parsing]
ptyxis_ref: src/ptyxis-tab.c, set_title handler
---

# Tab title follows OSC 0/2 sequence

Optional spec prose. One sentence for most features; full behavioral
description, edge cases, and divergence notes where warranted.

## Code Pointers
- `src/ptyxis-tab.c` — `set_title`: applies the parsed OSC title to the tab

## Test plan
- [x] OSC 2 with valid UTF-8 updates title — `tab::osc_title_updates`
- [x] Title persists across tab switch — `e2e::tab_title_osc` (waydriver)
- [ ] Invalid UTF-8 in title payload — uncovered

## Manual verification
- Title legible at fractional scaling — *manual: cannot assert rendering quality*
  - Setup: external monitor at 150% scaling, default profile
  - Steps:
    1. Open a tab
    2. `printf '\033]2;Ünïcödé tîtle\007'`
    3. Switch to another tab and back
  - Expect: crisp glyphs, no blur or clipping, in active and inactive states
````

## Frontmatter

Frontmatter is **standard YAML** between `---` fences, parsed by a real YAML
parser. Unlike earlier versions of this system — which restricted frontmatter
to flat `key: value` lines — custom fields may now use the full YAML feature
set: nested mappings, sequences, and multiline/block scalars (`|`, `>`).

The CLI's serializer still emits **canonical, minimal frontmatter**: core
fields first (`id`, `status`, `tags`), then remaining keys alphabetically,
with flat scalars and scalar lists rendered inline (`tags: [osc, tabs]`).
Complex custom values are written as block YAML under their key. Hand edits may
use any valid YAML; running a write command re-canonicalizes scalar fields and
may reflow the formatting (not the meaning) of complex values.

> **Quote any value containing a colon-followed-by-space (`: `).** In YAML
> `wontfix_reason: MVP scope: containers` parses the value as a nested mapping
> and fails verify; write `wontfix_reason: "MVP scope: containers"`. (A colon
> with no following space, like `ptyxis_ref: ptyxis-tab.c:1621`, is fine.) The
> CLI's own writers quote correctly; this bites files written by hand or by a
> script — verify's parse error includes a hint when it sees this shape. The
> bulk-import path below sidesteps it entirely, since JSON quotes every string.

| Field | Required | Rules |
|---|---|---|
| `id` | yes | `FEAT-NNNN`; must match filename; unique forever |
| `status` | yes | `planned` \| `partial` \| `implemented` \| `wontfix` (+ configured extras) |
| `tags` | yes | non-empty list, lowercase kebab-case, open vocabulary |
| `created` | no | RFC3339 datetime (e.g. `2026-06-16T14:30:00Z`); set once at creation; auto-maintained |
| `updated` | no | RFC3339 datetime; refreshed on every user-initiated write; auto-maintained |
| `references` | no | ID→title map of linked work items (and features); auto-maintained |
| `blocked_by` / `blocks` | no | ID→title maps of the blocker relation; auto-maintained (see Blockers) |
| `wontfix_reason` | iff wontfix | one-line ADR for the scope exception |
| `spec` | no | pointer to long-form shared material (a plain string field) |
| custom | per config | validated against `[fields.*]` declarations |

There is deliberately no `tests:` field — covering tests are derived from
the test plan, eliminating a sync surface.

The `references` map is **auto-maintained** by opys — it links a document to the
others it relates to and is kept bidirectional and title-fresh on every write.
You do not hand-edit it; a closed document leaves a struck-through (`~~title~~`)
tombstone here. Bare `PREFIX-NNNN` ID mentions
in body prose are rewritten into markdown links on sync.

### Custom-field type mapping

`[fields.<name>].type` is checked against the YAML type of the value:

| `type` | Accepts | Rejects |
|---|---|---|
| `string` | a YAML string node (`foo`, `"foo"`, or a block scalar) | bare booleans/numbers — quote them (`"true"`, `"123"`) to count as strings |
| `list` | a YAML sequence (elements may themselves be nested) | scalars, mappings |
| `bool` | `true` / `false` | the strings `"true"`/`"false"` |
| `int` | an integer | floats, booleans |
| `enum` | a string listed in the field's `values` | any string not in `values`, and non-strings |

An `enum` field must declare a non-empty `values` array (verify errors on an
empty one). Reserved keys (`id`, `status`, `tags`, `created`, `updated`,
`references`, `blocked_by`, `blocks`) are always allowed; every other key must be
declared under the type's
`[types.<name>.fields.*]` or verify rejects it. Richer YAML does not relax
the declare-or-fail rule.

### Filtering by field

`opys list` filters by `--tag` and `--status`, and additionally by any custom
field with repeatable `--field <key>=<value>` (ANDed together). A scalar field
matches on equality; a `list` field matches when it *contains* the value:

```sh
opys list --field priority=high                 # enum/scalar equality
opys list --status partial --field area=cli     # combine with status
opys list --field tag-list=osc --format ids     # list-membership match
```

`opys list --type <name>` restricts the listing to one document type.

## Blockers

Mark a dependency between two items (features and/or work items) with
`opys block <id> --by <blocker-id>`; `opys unblock <id> --by <blocker-id>`
removes it. The relation is **directional and bidirectional**: the blocked item
gains a `blocked_by` entry and the blocker gains the inverse `blocks` entry,
both kept title-fresh and sorted automatically (you do not hand-edit them).
Either id may be a document of any type.

Blocking a document whose type has a `blocked` status auto-sets it — the blocker
link itself serves as the `blocked_reason`, so none is required; `unblock`
reverts it to `in-progress` once no blocker (and no free-text reason) remains. A
type without a `blocked` status (e.g. `feature`) treats the link as purely
informational.

Blocker entries resolve, tombstone on close (`TASK-0042: ~~title~~`), and reserve
ids exactly like `references`; a closed blocker is therefore safe to leave in
place, and `opys cleanup` strips the struck entries.

## Status semantics

- `planned` — in inventory, no implementation.
- `partial` — some behavior present; unchecked test-plan items document the gap.
- `implemented` — complete; requires ≥1 checked test-plan item.
- `wontfix` — deliberate exception, reason required; stays in the inventory so
  the decision is recorded and not re-litigated.

Status changes go through `set-status`, never hand edits — the guards live
there.

## Test plan rules

- A test-plan item is a *behavioral case*, not a single test. `[x]` means "at
  least one automated test covers this case" — plan-state, not run-results.
- The structure is just a `checklist` section; a [section check](#section-checks)
  (below) is what makes a checked item carry a resolvable reference. The default
  config attaches one whose `pattern` parses a `` `module::test_name` `` span and
  whose `must_match` greps the test name under `src`/`tests`. A case may list
  **several** refs, and one test may be referenced by several cases.
- Because the ref shape is the check's `pattern` (here, backtick spans containing
  `::`), a checked item may carry other inline code in its *prose* — a shell
  snippet, an escape sequence, a literal argument — without it being mistaken for
  a reference. (`` `ssh -t … exec $SHELL` `` is prose; `` `app.rs::sftp_rewrite` ``
  matches the pattern.)
- Unit vs e2e is not a structural boundary — annotate informally, e.g.
  `(waydriver)`.
- Items are permanent. Once covered, shorten — never delete. The enumeration
  of cases is what makes a plan reviewable for completeness; a plan showing
  only gaps cannot be reviewed, and it is how you catch an implementation
  that covered three of seven edge cases.

## Section checks

Any section of any type may carry a list of `[[types.<name>.sections.checks]]`
— a **universal, config-driven content check** run at `verify` time. Each check
declares a `pattern` regex that parses one body line into **named capture
groups**, then asserts those captures point at something real via `file` and/or
`must_match`:

```toml
[[types.feature.sections.checks]]
pattern    = '`(?P<file>[^`]+\.rs)` — `(?P<sym>[^`]+)`'   # parse → named groups
file       = "file"                  # capture group naming a file to open
roots      = ["src"]                 # resolve file / corpus against these (default ["."])
must_match = '${sym}'                # regex; ${group} = the regex-escaped capture
scope      = "all"                   # "all" (every line) | "checked" (checked items)
message    = "`${sym}` not found in `${file}`"   # optional custom failure message
```

- **`scope = "all"`** (default): every line of the section is scanned; lines not
  matching `pattern` are skipped as prose. Each match is validated.
- **`scope = "checked"`**: only checked checklist items are scanned, and a checked
  item with **zero** `pattern` matches is itself an error (this is how the test
  plan requires every checked case to carry a reference). Only valid on a
  `checklist` section.
- For each match: if `file` is set, the captured path is resolved as
  `<root>/<capture>` over `roots` (project-root relative) and must exist. If
  `must_match` is set, its `${group}` placeholders are replaced by the
  **regex-escaped** captures and the resulting regex must match ≥1 time — in the
  opened file when `file` is set, otherwise in the concatenated corpus of all
  files under `roots`.
- `file` alone is a pure existence pointer; `must_match` alone is a corpus grep;
  both together pin a symbol to a specific file. At least one must be set.
- `message` (optional) customizes the failure text for a `must_match` miss;
  `${group}` is inserted **raw** (a missing `file` always reports
  `file '<path>' not found`). The example above is the real "Code Pointers"
  pattern — each `` `file` — `symbol` `` line must name a file that exists and
  contain that symbol.

The corpus grep is language-agnostic but **unsound** — it matches any occurrence
(a comment or string literal containing the name passes). For a precise check,
point `file` at the defining file and make `must_match` a definition pattern
(e.g. `fn ${name}\b`).

## Manual verification rules

Manual verification is independent of automation — it is not reserved for the
unautomatable. A manual item may re-check, in a user-friendly way, behavior
that automated tests already cover (an end-to-end sanity pass), or it may be
the only coverage a case has.

- Plain list items, never checkboxes — manual cases have no in-file state;
  they are executed per release and results live in CI or commit history.
- Each item: a one-line description, then a sublist with `Setup:` (single
  bullet — preconditions), `Steps:` (numbered — the sequence), `Expect:`
  (single bullet — a judgment-free pass criterion). If a crisp Expect cannot
  be written, the case is under-specified.
- **Automated-coverage signal:** add ≥1 backticked test ref on the item's
  description line to mark it as also automated. Items with **no** ref have no
  automated coverage — `opys stats` counts them, since they are the most important
  to run by hand. When an item exists *because* it cannot be automated, say so
  in the description
  (e.g. *manual: cannot assert rendering quality*) so the reason is recorded.
- Write for a competent operator who knows the project but not this case:
  assume they can run the app; spell out exact escape sequences, config
  values, and the precise defect to look for.
- Procedures longer than ~10 lines or shared across features move to a shared
  doc and are referenced.

## Bulk creation and migration

`opys new` creates one feature per process and runs a full sync each time
— fine interactively, far too slow for migrating hundreds or thousands of
features. Two supported bulk paths avoid that:

- **`opys import <file.jsonl>`** — one JSON object per line, each describing a
  feature. `title` and `tags` are required; `status` (default `planned`),
  `spec`, and any declared custom fields are optional top-level keys; `body` is
  optional markdown placed under the `# Title` heading (use it to carry a
  `## Test plan` or `## Manual verification`). One ID allocation and one sync
  cover the whole batch, and it is **transactional** — if any record is
  rejected, nothing is written. The same write-time status guards as `new`
  apply (a record with `"status": "implemented"` must include a checked
  test-plan item in its `body`). Run `opys verify` afterwards for the deep
  checks (tag shape, reference resolution, field types). Example line:

  ```json
  {"title": "Tab title follows OSC 0/2", "tags": ["osc", "tabs"], "status": "implemented", "ptyxis_ref": "src/ptyxis-tab.c", "body": "## Test plan\n- [x] OSC 2 updates title — `tab::osc_title`"}
  ```

- **Write the files directly** — `opys` reads plain markdown, so you can emit
  canonical `FEAT-NNNN.md` files yourself (matching the frontmatter rules
  above), then run `opys sync` once and `opys verify`. This is a fully
  supported escape hatch when your source data does not map cleanly onto the
  JSONL schema; allocate IDs monotonically and never reuse a retired one.

Either way: do **not** loop `opys new` over a large migration. After import,
review in batches per tag using `opys list`, exactly as for a hand-built
inventory.

## What never goes in feature files

Test results, execution dates, completion claims, assignees, priorities, or
sprint metadata. CI owns automated results; this system owns intent only.

Implementation logs, task checklists, and branch/PR links also do not belong
here — they go in an ephemeral document (a `task`/`bug`/`chore`), which is
deleted when the change lands. The feature is permanent; the work item is
throwaway.
