# Document file format (normative)

## Layout

The inventory lives under a base directory, default `docs/opys/` (configurable
with `--dir` / `OPYS_DIR`), so it stays out of the repo root:

```
docs/opys/
  opys.toml             # the configuration (see below) — declares the types
  _retired.txt          # append-only log of deleted IDs (never reused), sorted
  items/                # default directory for documents (FEAT-0001.md, TASK-0002.md, …)
  views/                # generated — never hand-edit
  runbooks/             # dated manual-runbook instances, committed after execution
  INDEX.md              # generated — never hand-edit
```

Each document is one markdown file named after its ID (`FEAT-0001.md`,
`EPIC-0003.md`). **A document's type is its ID prefix.** By default every type's
files live together in `items/`; a type may set its own `dir` (`epic` →
`epics/`). Directory structure must never encode taxonomy — that is what tags and
generated views are for. If a directory becomes unwieldy (~2000+ files), shard
mechanically by ID prefix; sharding is cosmetic, tooling treats the tree as flat.

## Configuration: `docs/opys/opys.toml`

One config declares every document **type** and the rules over them. `opys config
init` writes the opinionated default (a permanent `feature` type plus ephemeral
`task`/`bug`/`chore`); `opys config validate` checks it.

```toml
pad = 4                              # zero-padding width for the numeric id part

[tests]                              # test-reference resolution (test-plan sections)
search_paths = ["src", "tests"]
reference_check = "grep"             # "grep" | "extract" | "none"
# name_pattern = "fn\\s+(\\w+)\\s*\\("   # required for "extract"

[report]
parity = false                       # report feature-parity % (parity projects)

[types.feature]                      # one [types.<name>] block per document type
prefix = "FEAT"                      # ^[A-Z][A-Z0-9]*$, unique across types
# dir = "features"                   # default: the shared items/
statuses = ["planned", "partial", "implemented", "wontfix", "archived"]
default_status = "planned"
terminal_statuses = []               # statuses reached only via `close` (which deletes)
tags_required = true
# requires_link = { to = "feature", min = 1 }   # must reference ≥min docs of a type

[types.feature.fields.priority]      # custom frontmatter fields (the closed schema)
type = "enum"                        # string | list | bool | int | enum
values = ["low", "high"]             # an enum constrains the value
# pattern = '^JIRA-[0-9]+$'          # a string field may require a regex

[[types.feature.sections]]           # required/known body sections, by kind
heading = "Test plan"
kind = "test-plan"                   # prose | log | checklist | test-plan | manual
# required = true

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
| `references` | no | ID→title map of linked work items (and features); auto-maintained |
| `blocked_by` / `blocks` | no | ID→title maps of the blocker relation; auto-maintained (see Blockers) |
| `wontfix_reason` | iff wontfix | one-line ADR for the parity/scope exception |
| `spec` | no | path to long-form shared material; must resolve |
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
empty one). Reserved keys (`id`, `status`, `tags`, `references`, `blocked_by`,
`blocks`) are always allowed; every other key must be declared under the type's
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
Either id may be a feature or work-item id.

Blocking a **work item** auto-sets its status to `blocked` — the blocker link
itself serves as the `blocked_reason`, so none is required; `unblock` reverts it
to `in-progress` once no blocker (and no free-text reason) remains. Features have
no `blocked` status, so a blocked feature is purely an informational link.

Blocker entries resolve, tombstone on close (`TASK-0042: ~~title~~`), and reserve
ids exactly like `references`; a closed blocker is therefore safe to leave in
place, and `work-item cleanup` strips the struck entries.

## Status semantics

- `planned` — in inventory, no implementation.
- `partial` — some behavior present; unchecked test-plan items document the gap.
- `implemented` — complete; requires ≥1 checked test-plan item.
- `wontfix` — deliberate exception, reason required; stays in the inventory so
  parity accounting is honest and the decision is not re-litigated.

Status changes go through `set-status`, never hand edits — the guards live
there.

## Test plan rules

- A test-plan item is a *behavioral case*, not a single test. `[x]` means "at
  least one automated test covers this case" — plan-state, not run-results.
- Every checked item ends with ≥1 backticked test reference; verify confirms
  each exists (see below). A case may list **several** refs (covered by several
  tests), and one test may legitimately be referenced by several cases.
- Reference format: `module::test_name`, or `path/to/file::test_name` when the
  project uses `extract` mode and you want to pin the test to its file.
- **Only backtick spans containing `::` are parsed as references.** A checked
  item may therefore carry other inline code in its *prose* — a shell snippet,
  an escape sequence, a literal argument — without it being mistaken for a test
  reference. (`` `ssh -t … exec $SHELL` `` is prose; `` `app.rs::sftp_rewrite` ``
  is the reference.)
- Unit vs e2e is not a structural boundary — annotate informally, e.g.
  `(waydriver)`.
- Items are permanent. Once covered, shorten — never delete. The enumeration
  of cases is what makes a plan reviewable for completeness; a plan showing
  only gaps cannot be reviewed, and it is how you catch an implementation
  that covered three of seven edge cases.

### How references are validated (`test_reference_check`)

- `"grep"` (default): the test name (the part after the last `::`) must appear
  as a substring somewhere under `test_search_paths`. Language-agnostic but
  **unsound** — it matches any occurrence, so a comment, a string literal, or
  another test's body that happens to contain the name passes. Use it only
  before `extract` is set up; prefer `extract` once tests exist.
- `"extract"`: `test_name_pattern` (a regex with one capture group) extracts
  the real test names from every file under `test_search_paths`, so a reference
  resolves against a *defined test*, not any substring. The strongest option.
  How a reference's prefix is classified:
  - **Module ref** — the prefix has no `/` or `.` (e.g. `window::grid_px`):
    `name` need only appear among the extracted names anywhere under the search
    paths.
  - **Path ref** — the prefix contains `/` or `.` (e.g. `window.rs::grid_px` or
    `src/window.rs::grid_px`): the file is resolved relative to the project
    root *and* to each `test_search_paths` entry, and `name` must be defined in
    that file. So a bare `window.rs::name` resolves `src/window.rs` when that is
    where it lives; write `src/window.rs::name` to pin the file unambiguously.
- `"none"`: skip existence checking (e.g. before any tests exist).

## Manual verification rules

Manual verification is independent of automation — it is not reserved for the
unautomatable. A manual item may re-check, in a user-friendly way, behavior
that automated tests already cover (an end-to-end sanity pass), or it may be
the only coverage a case has.

- Plain list items, never checkboxes — manual cases have no in-file state;
  they are executed per release and results live in runbook instances.
- Each item: a one-line description, then a sublist with `Setup:` (single
  bullet — preconditions), `Steps:` (numbered — the sequence), `Expect:`
  (single bullet — a judgment-free pass criterion). If a crisp Expect cannot
  be written, the case is under-specified.
- **Automated-coverage signal:** add ≥1 backticked test ref on the item's
  description line to mark it as also automated. Items with **no** ref have no
  automated coverage — `report` counts them and `manual-runbook` flags them ⚠
  and lists them first, since they are the most important to run by hand. When
  an item exists *because* it cannot be automated, say so in the description
  (e.g. *manual: cannot assert rendering quality*) so the reason is recorded.
- Write for a competent operator who knows the project but not this case:
  assume they can run the app; spell out exact escape sequences, config
  values, and the precise defect to look for.
- Procedures longer than ~10 lines or shared across features move to a shared
  doc and are referenced.

## Bulk creation and migration

`opys new` creates one feature per process and regenerates `INDEX.md` + all
`views/` each time — fine interactively, far too slow for migrating hundreds or
thousands of features. Two supported bulk paths avoid that:

- **`opys import <file.jsonl>`** — one JSON object per line, each describing a
  feature. `title` and `tags` are required; `status` (default `planned`),
  `spec`, and any declared custom fields are optional top-level keys; `body` is
  optional markdown placed under the `# Title` heading (use it to carry a
  `## Test plan` or `## Manual verification`). One ID allocation and one view
  sync cover the whole batch, and it is **transactional** — if any record is
  rejected, nothing is written. The same write-time status guards as `new`
  apply (a record with `"status": "implemented"` must include a checked
  test-plan item in its `body`). Run `opys verify` afterwards for the deep
  checks (tag shape, reference resolution, field types). Example line:

  ```json
  {"title": "Tab title follows OSC 0/2", "tags": ["osc", "tabs"], "status": "implemented", "ptyxis_ref": "src/ptyxis-tab.c", "body": "## Test plan\n- [x] OSC 2 updates title — `tab::osc_title`"}
  ```

- **Write the files directly** — `opys` reads plain markdown, so you can emit
  canonical `FEAT-NNNN.md` files yourself (matching the frontmatter rules
  above), then run `opys sync-views` once and `opys verify`. This is a fully
  supported escape hatch when your source data does not map cleanly onto the
  JSONL schema; allocate IDs monotonically and never reuse a retired one.

Either way: do **not** loop `opys new` over a large migration. After import,
review in batches per tag using the generated views, exactly as for a
hand-built inventory.

## What never goes in feature files

Test results, execution dates, completion claims, assignees, priorities, or
sprint metadata. CI owns automated results; committed runbook instances own
manual results; this system owns intent only.

Implementation logs, task checklists, and branch/PR links also do not belong
here — they go in an ephemeral document (a `task`/`bug`/`chore`), which is
deleted when the change lands. The feature is permanent; the work item is
throwaway.
