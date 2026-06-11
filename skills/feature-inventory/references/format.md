# Feature file format (normative)

## Layout

The inventory lives under a base directory, default `docs/` (configurable with
`--dir` / `OPYS_DIR`), so it stays out of the repo root:

```
docs/
  features/
    _config.toml        # project configuration (see below)
    _retired.txt        # append-only log of deleted IDs (never reused)
    PREFIX-0001.md
    ...
    INDEX.md            # generated — never hand-edit
  views/                # generated — never hand-edit
  runbooks/             # dated manual-runbook instances, committed after execution
```

If `ls docs/features/` becomes unwieldy (~2000+ files), shard mechanically by
ID prefix (`docs/features/04/PREFIX-0421.md`). Sharding is cosmetic only;
tooling treats the tree as flat. Directory structure must never encode taxonomy.

## Configuration: `docs/features/_config.toml`

```toml
prefix = "VIK"                      # -> VIK-0001
pad = 4                             # zero-padding width
test_search_paths = ["src", "tests"]
test_reference_check = "grep"       # "grep" | "extract" | "none"
# test_name_pattern = "fn\\s+(\\w+)\\s*\\("  # required for "extract" mode
extra_statuses = []                 # beyond the four core statuses
parity = false                      # report feature-parity % (parity projects only)

[fields.ptyxis_ref]                 # per-project custom frontmatter fields
type = "string"                     # string | list | bool | int
required = false
description = "Pointer into Ptyxis source establishing reference behavior"
```

Custom fields are the per-project extension point. A field used in any
feature file must be declared here or verify fails — undeclared fields are
how schema drift starts. `opys schema --kind config` and `--kind frontmatter`
emit JSON Schemas (the frontmatter one is derived from your declared fields)
for editor (Even Better TOML) or CI validation.

## A complete feature file

````markdown
---
id: VIK-0421
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

| Field | Required | Rules |
|---|---|---|
| `id` | yes | `PREFIX-NNNN`; must match filename; unique forever |
| `status` | yes | `planned` \| `partial` \| `implemented` \| `wontfix` (+ configured extras) |
| `tags` | yes | non-empty list, lowercase kebab-case, open vocabulary |
| `wontfix_reason` | iff wontfix | one-line ADR for the parity/scope exception |
| `spec` | no | path to long-form shared material; must resolve |
| custom | per config | validated against `[fields.*]` declarations |

There is deliberately no `tests:` field — covering tests are derived from
the test plan, eliminating a sync surface.

### Custom-field type mapping

`[fields.<name>].type` is checked against the YAML type of the value:

| `type` | Accepts | Rejects |
|---|---|---|
| `string` | a YAML string node (`foo`, `"foo"`, or a block scalar) | bare booleans/numbers — quote them (`"true"`, `"123"`) to count as strings |
| `list` | a YAML sequence (elements may themselves be nested) | scalars, mappings |
| `bool` | `true` / `false` | the strings `"true"`/`"false"` |
| `int` | an integer | floats, booleans |

Reserved fields (`id`, `status`, `tags`, `spec`, `wontfix_reason`) are always
allowed; every other key must be declared under `[fields.*]` or verify rejects
it. Richer YAML does not relax the declare-or-fail rule.

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
- Unit vs e2e is not a structural boundary — annotate informally, e.g.
  `(waydriver)`.
- Items are permanent. Once covered, shorten — never delete. The enumeration
  of cases is what makes a plan reviewable for completeness; a plan showing
  only gaps cannot be reviewed, and it is how you catch an implementation
  that covered three of seven edge cases.

### How references are validated (`test_reference_check`)

- `"grep"` (default): the test name (the part after the last `::`) must appear
  as a substring somewhere under `test_search_paths`. Language-agnostic but
  weak — a comment mentioning the name passes.
- `"extract"`: `test_name_pattern` (a regex with one capture group) extracts
  the real test names from every file under `test_search_paths`. A
  `module::name` ref must match an extracted name; a `path::name` ref must
  resolve the file *and* find `name` defined in it. Strongest option.
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

## What never goes in feature files

Test results, execution dates, completion claims, assignees, priorities, or
sprint metadata. CI owns automated results; committed runbook instances own
manual results; this system owns intent only.
