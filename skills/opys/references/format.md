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
    _retired.md         # frontmatter map: reserved IDs -> title (never reused)
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
encode taxonomy — that is what tags and `opys list` are for. A document's path
always stays under the base: a write that would place a file outside it is
refused at flush time.

### The retired ledger (`_retired.md`)

`_retired.md` at the base is the reserved-id ledger: a document-shaped markdown
file whose frontmatter holds a single `retired:` map of `ID: last title`, sorted
by number. Every id it lists is reserved forever — allocation continues past it,
and `verify` rejects a live document reusing one. It is written by `retire`,
`close`, `renumber`, and `cleanup` (which first ledger-reserves any id whose
last struck tombstone it is about to strip). The value is the document's last title, for human reference
only; git records the when and why (`retire --reason` is echoed, not persisted).

**An unreadable ledger is an error, never an empty ledger**: if `_retired.md`
(or the legacy file, below) exists but cannot be read or parsed, `verify`
reports it as a problem, and every command that allocates ids, reserves ids, or
flushes files refuses to run (and never rewrites the ledger) until the file is
fixed — an empty read would reuse reserved numbers and the next rewrite would
drop them permanently. Read-only commands (`list`, `show`, `query`) still
work. A pre-0.12 plaintext `_retired.txt` is read only when `_retired.md` is
absent, and is migrated to it on the next write.

Retiring or closing a document that another live document *requires* a link to
(a `requires_link`/`require_link` rule) leaves that rule failing — a struck
tombstone is not a live link; `retire` warns about each such document so the
corpus never goes red silently. To undo a mistaken close/retire, restore the
file from git and delete its line from `_retired.md` — the one sanctioned hand
edit of the ledger.

### Id allocation

Ids are numbers from **one global, monotonically increasing sequence** shared
by every type: `new` takes the next number above everything it has seen — live
documents, the retired ledger, and every relation-map entry (struck or not) —
so a number is never reused, and a gap is harmless (monotonicity, not density,
is the contract).

By default the sequence is dense. A project that allocates on several branches
at once (parallel agents, feature branches that file their own tasks and bugs)
sets a **modulus** instead:

```toml
[ids]
modulus = 16
```

`M` must be at least 2 (one slot cannot interleave) and at most `2^32`
(`4294967296` — ids are stored as 64-bit numbers, and a larger stride could
walk past that range); `config validate` and `verify` report a modulus outside
those bounds, and every allocating command refuses to run under one. The
`[ids]` table accepts only `modulus`: any other *key* inside it (a misspelled
`modulus`, say) is a TOML parse error that stops every command with exit 2,
like a syntax error, rather than leaving the project silently dense. A
misspelled *table* name (`[idz]`) is another matter: unknown top-level tables
are ignored, here as for `[layout]` or `[file_refs]`, so such a typo does leave
allocation dense, silently. `opys config validate` is the tell: under a modulus
it prints the resolved slot after the `OK` line (`ids: slot 2 of 4 (from
OPYS_ID_SLOT) — next id here is congruent to 2 mod 4`) and says nothing when
allocation is dense. It resolves the slot the way an allocating command would,
so an unusable `OPYS_ID_SLOT` exits 2 there too, before anything is printed.

With `modulus = M` every allocator owns a **slot** `k` in `0..M` and takes only
numbers congruent to `k` modulo `M`: the smallest `n ≡ k (mod M)` above the
corpus-wide max, then `n + M`, `n + 2M`, … Two allocators with different slots
cannot collide, on any branches, with no shared ledger, ref, or push. The slot
comes from:

1. `OPYS_ID_SLOT` when set — an integer below `M`; anything else, an empty
   value included, is a usage error (exit 2) naming the slot and the modulus
   (so `OPYS_ID_SLOT="$SLOT"` with an unassigned `$SLOT` fails instead of
   silently sharing one slot). An orchestrator running several agents sets a
   distinct slot per agent.
2. Otherwise a stable hash of `user@host`: FNV-1a 64-bit (offset basis
   `0xcbf29ce484222325`, prime `0x100000001b3`) of the UTF-8 bytes, modulo
   `M`. The user is `$USER`, else `$USERNAME`, else `unknown`; the host is
   `$HOSTNAME`, else `$COMPUTERNAME`, else `/etc/hostname`, else `localhost`.
   Every one of those values is trimmed of surrounding whitespace, and a
   variable that is set but empty or blank (or an empty `/etc/hostname`)
   counts as unset — unlike `OPYS_ID_SLOT`, where an empty value is an error
   (it is trimmed before it is parsed, so `" 3 "` is slot 3). No command is
   run to find the host, so the slot stays reproducible from the raw
   environment by any tool that applies the same trimming and fallbacks. The
   same person on the same machine therefore always lands in the same slot —
   with one caveat: **on macOS neither `HOSTNAME` nor `/etc/hostname` is
   normally there**, so everyone on such a machine hashes as `user@localhost`,
   and two same-named accounts on two Macs share a slot. Export `HOSTNAME`, or
   set `OPYS_ID_SLOT` (which an orchestrator does anyway).

Which commands read it: the slot is resolved and validated **before the
inventory is loaded**, and only by the commands that can allocate — `new`,
`import`, `renumber`, and the node's `new` action. (`config validate` resolves
it as well, but only to print the line above; it allocates nothing.) So a bad
`OPYS_ID_SLOT` fails fast, without taking the inventory lock or making a
concurrent writer wait. Reads (`show`, `list`, `next`, `query`, `stats`,
`verify`) and the non-allocating writes (`set-status`, `apply`, `tag`, `log`,
`block`, `unblock`, `close`, `retire`, `cleanup`, `sync`) never look at the
variable at all.

Two allocators sharing a slot are a misconfiguration; they collide exactly as
they would without a modulus, and `opys renumber` repairs it exactly as today.
`verify` is unchanged: number uniqueness and the never-reuse ledger are the
invariants interleaving preserves, so a corpus merged from several slots passes
as is. `import` reserves its batch as one block that steps by `M` (its summary
says so: `imported 3 feature document(s): FEAT-0005..FEAT-0013 step 4` — the
range is not dense), and `renumber` draws each replacement id through the same
slot.

Consequences to plan for: one allocator's consecutive ids are `M` apart
(`M - 1` unused numbers lie between them), so choose `pad` for the sequence,
not the document count — at `M = 16`, `pad = 4`
holds roughly six hundred documents per slot (`9999 / 16`) before ids grow a
fifth digit (which is allowed: `pad` is a minimum width, never a cap). A
modulus can be set or changed at any time: existing ids simply stay below the
max and every allocator only ever allocates above everything it sees — what
changes is which allocators share a slot from then on, so treat a change as a
new assignment of slots. With no modulus configured, allocation is the dense
sequence, byte-identically, and `OPYS_ID_SLOT` is not read.

## Configuration: `opys.toml`

One config declares every document **type** and the rules over them. `opys config
init` writes the opinionated default (a permanent `feature` type plus ephemeral
`task`/`bug`/`chore`); `opys config validate` checks it.

```toml
pad = 4                              # zero-padding width for the numeric id part

# [ids]                              # interleaved ids for multi-branch work
# modulus = 16                       # one slot per allocator (OPYS_ID_SLOT, else user@host)

# [layout]                           # on-disk path template (relative to base)
# path = "{type}/{status}/{id}.md"   # default; {type}/{status} empty → flat

# [git]                              # reading the inventory out of git
# main = "main"                      # the truth branch (falls back to "master")
# branches = ["refs/heads/*"]        # which refs count; remotes are opt-in

[types.feature]                      # one [types.<name>] block per document type
prefix = "FEAT"                      # ^[A-Z][A-Z0-9]*$, unique across types
# dir = "features"                   # the {type} layout segment; default empty (flat)
statuses = ["planned", "partial", "implemented", "wontfix", "archived"]
default_status = "planned"
terminal_statuses = []               # statuses reached only via `close` (which deletes)
status_dirs = { archived = "_archived" }   # per-status {status} segment; default empty
tags_required = true
# requires_link = { to = "feature", min = 1 }   # must reference ≥min docs of a type
releases_blocks = ["implemented", "wontfix", "archived"]   # stops blocking here (see Blockers)
order_by = ["priority", "blocks", "id"]     # tiebreak chain (see Ordering); this is the default

[types.feature.transitions]          # optional status graph; omit it for any-to-any
planned     = ["partial", "implemented", "wontfix"]
partial     = ["planned", "implemented", "wontfix"]
implemented = ["partial", "planned", "archived"]   # `planned` is the escalation edge
wontfix     = ["planned"]            # a status absent from the map is a dead end

[types.feature.ttl]                  # optional: how long a status may hold a doc
planned = "2y"                       # a bare duration is observed, never fixed
partial = { after = "90d", then = "planned" }   # `verify --fix` performs this

[types.feature.max_visits]           # optional: how often a status may be entered
partial = { limit = 8, then = "planned" }

[types.feature.fields.risk]          # custom frontmatter fields (the closed schema)
type = "enum"                        # string | list | bool | int | enum
values = ["low", "high"]             # an enum constrains the value
# pattern = '^JIRA-[0-9]+$'          # a string field may require a regex

[types.feature.fields.priority]      # the field `order_by`'s priority key reads
type = "int"                         # ADR-0046: opt-in, lower = higher

[[types.feature.sections]]           # required/known body sections, by kind
heading = "Test plan"
kind = "checklist"                   # prose | log | checklist | structured
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

[[rules]]                            # `when.tag` gates on a tag (exact or key)
when = { tag = "needs-review" }      # matches `needs-review`, `needs-review:ui`, …
require_field = "reviewer"
```

A frontmatter field used on any document must be declared on its type or verify
fails — undeclared fields are how schema drift starts. An `enum` constrains its
value to the declared `values`; a string field may carry a `pattern`. The closed
assertion set for `[[rules]]` is `require_field`, `field_matches`,
`require_section`, `require_checked_section`, `require_link`, `require_any`
(whose terms are `{field}`, `{link}`, `{section}`, or `{tag}`), and
`require_log_since_status` (see [Log sections](#log-sections)).
`opys list --field <key>=<value>` filters by any custom field (see below).

### `[git]` — reading branches (optional)

opys reads the inventory as it stands on other branches and worktree heads,
in-process through gix and never by spawning `git`. **Main is truth** — the
baseline every other ref is diffed against; everything else is *progress*.

```toml
[git]
main = "main"                        # the truth branch; falls back to "master"
branches = ["refs/heads/*"]          # which refs count (default: local heads)
```

`main` names the truth branch. When `refs/heads/<main>` does not resolve the
reader falls back to `master`, so a repository that never renamed its default
branch needs no `[git]` table at all.

The `[git]` table accepts only `main` and `branches`: any other *key* inside it
(a misspelled `branches`, say) is a TOML parse error that stops every command
with exit 2, like a syntax error, rather than silently reading the refs you did
not mean. As with `[ids]`, a misspelled *table* name (`[gti]`) is another matter
— unknown top-level tables are ignored — so such a typo does leave the defaults
in force, silently.

`branches` is a list of glob patterns matched against **fully qualified** ref
names with git's own wildmatch, in which `*` crosses `/`: `refs/heads/*` covers
`refs/heads/feat/x`, exactly as a refspec does. The default reads local heads
only; add `"refs/remotes/*"` to include remote-tracking refs, which are opt-in
because keeping them fetched is your job and a stale one reports work that ended
days ago. Every **linked worktree head** is read whatever the patterns say (a
worktree head is not a ref name, so no pattern could select it) — both the
symbolic `HEAD` plain git writes and the bare object id a jj-colocated checkout
leaves behind.

Refs are deduplicated **by commit** — one commit is one column — and labelled by
the branch name when one points at that commit, else by the checkout directory
names of the worktrees parked on it, sorted and joined. The truth branch is never
a column of its own. A **checkout** is named the same way: the branch its `HEAD`
points at, or — for the detached head every jj-colocated workspace carries — the
branch whose tip it is parked on, so a checkout is not nameless just because git
would call it detached. Each ref is diffed against main under the inventory base,
keyed by **filename** (which is the document's id, so the relocation a status
change causes is not read as a delete plus an add), and only the blobs that
differ are decoded, through the real parser.

A project that is not a git repository — or one whose truth branch does not
exist, or an `opys` built without the `git` feature — is main-only and says so
with a single notice. A repository that *is* there and cannot be opened (a
half-written `.git/config`, an unreadable `.git` directory, an IO error) says so
too, in its own words and naming what it could not read, and is marked as worth
trying again rather than settled: it is a passing state, not a verdict. That
second state is decided by probing the `.git` the search walked past, not by
what the search reported, because the search cannot tell the two apart either —
it treats a `.git` it cannot read as one that is not there. Nothing fails.

That is the *reader*, which always has a main-only answer to give. `opys history
<id>` has none: it reports the committed timeline and nothing else, so outside a
repository it says so and exits 2, and a binary built without the `git` feature
answers the command by naming the feature it lacks (also exit 2) rather than
reporting an empty history it never looked for. The subcommand exists in every
build for exactly that reason.

### `[file_refs]` — code mentions of an id (optional)

How a document id is written in **code** (comments, strings, identifiers), so
`opys show <id> --refs` can list those mentions and `opys renumber` can warn when
a renumbered id is still referenced. This is distinct from the `references`
relation maps *between documents* — here the target is arbitrary source.

```toml
[file_refs]
roots = ["src", "tests"]             # where to scan (project-root relative; default ["."])
formats = [
    { template = "{id}" },               # FEAT-0001  (the canonical id)
    { template = "{prefix}{num}" },       # FEAT1
    { template = "{prefix_lower}_{num}" }, # feat_1
    # { template = "...", word = false },  # match a substring, not a whole word
]
```

Each format's `template` is rendered for an id with the placeholders `{id}` (the
full `PREFIX-NNNN`), `{prefix}` / `{prefix_lower}`, `{num}` (the number with no
padding), and `{padded}` (zero-padded to `pad`). By default a match must fall on
word boundaries; `word = false` matches a substring. With no `formats` configured
only the canonical `{id}` is scanned. The inventory base, `.git`, `target`,
`node_modules`, and hidden directories are always skipped. `config validate`
rejects an unknown placeholder and a template with no number placeholder
(`{id}`/`{num}`/`{padded}`), which could not distinguish two ids sharing a prefix.

`opys show <id> --refs` prints the document, then a `--- file references ---`
block listing every `path:line: text` hit. `opys renumber`, after renumbering,
scans for the **old** ids (it rewrites documents, not code) and — if any remain —
prints a warning with a suggested `sed -i` command per file, so a human or agent
can fix references selectively.

### `[[stats]]` — custom stats sections (optional)

`opys stats` renders the configured `[[stats]]`
sections. There is no hardcoded report: each section is a single **`sql`** query
run against an in-memory, throwaway relational view of the corpus; its result
set is rendered as a markdown table headed by `name`. The engine is
[GlueSQL](https://github.com/gluesql/gluesql) (a pure-Rust, in-memory SQL
engine). The default config ships three (status-by-type, tags, coverage);
replace or extend them freely.

On a terminal, `opys stats` renders that markdown styled (headed sections,
aligned/bordered tables, emphasis). When output is piped or redirected — or with
`--plain`, or when `NO_COLOR` is set — it emits raw markdown instead, so it stays
clean for grepping or feeding another tool.

```toml
[[stats]]
name = "Status by type"              # the table heading
sql = '''
SELECT type, status, COUNT(*) AS count,
       ROUND(100.0 * COUNT(*) /
             (SELECT COUNT(*) FROM docs d2 WHERE d2.type = d1.type)) AS pct
FROM docs d1
GROUP BY type, status
ORDER BY type, status
'''
```

The query runs over four tables, all rebuilt on every run from the live corpus
(read-only — a stat may not mutate them):

| Table | Columns |
|---|---|
| `docs` | `id, num, type, status, title, created, updated` (one row per document) |
| `tags` | `doc_id, tag, key, value` (one row per tag; `value` is `NULL` for a plain tag; `tag` is the raw form) |
| `sections` | `doc_id, heading, kind, items, unchecked` (only *countable* sections that are present — checklist/log/structured item counts; `unchecked` is checklist-only) |
| `fields` | `doc_id, key, value` (one row per declared custom field; one row per element for a list field) |

Any SQL GlueSQL supports works (`GROUP BY`, subqueries, `CASE`, `JOIN`, …). The
`SELECT` column labels become the table's column headers. `config validate` runs
each query against an empty corpus (catching parse errors and unknown
tables/columns that resolve without rows), and `verify` runs them against the
live corpus — a query that fails to parse or execute, or that is not a `SELECT`,
fails CI.

**Custom formatting with `template`.** A markdown table suits many-row results
(`Tags by key`), but not a single-row summary. Add an optional **`template`** and
each result row is rendered through it instead — `{column}` is replaced by that
row's value for that column (`{{`/`}}` for literal braces), and the rendered rows
are joined under the `## name` heading. So SQL stays the execution layer while you
own the presentation:

```toml
[[stats]]
name = "Ptyxis parity"
sql = "SELECT implemented, features, ROUND(100.0*implemented/features) AS pct FROM (…)"
template = "Implemented: {implemented} / {features} ({pct}%)"   # one row → one line

[[stats]]
name = "By status"
sql = "SELECT status, COUNT(*) AS c FROM docs GROUP BY status ORDER BY status"
template = "- {status}: {c}"                                    # many rows → a bullet list
```

A `template` may be multi-line (a single-row stat can render several lines). Every
`{placeholder}` must name a column the query selects — checked at `config
validate`/`verify` time against the query's columns (so it's caught even when the
corpus is empty), alongside malformed braces. Omit `template` to get the default
table.

**Performance note.** GlueSQL is a simple executor (no indexes or query planner).
`JOIN … ON` and `GROUP BY` are fine, but two patterns run as O(rows × corpus)
nested loops and get very slow on large inventories:

- `x IN (SELECT …)` / `x NOT IN (SELECT …)` — re-runs the subquery per outer row.
  Rewrite as a `JOIN` (for `IN`) or a `LEFT JOIN … WHERE joined.key IS NULL`
  anti-join (for `NOT IN`).
- `FROM (SELECT …)` derived tables — re-evaluated repeatedly.

Prefer a single `JOIN` pass with **conditional aggregation** —
`SUM(CASE WHEN … THEN … ELSE 0 END)` — and compute ratios inline from those
aggregates rather than wrapping the query in an outer `SELECT`. In practice this
turns a multi-second stat into a sub-100ms one.

## A complete feature file

The three auto-maintained keys (`created`, `updated`, `history`) are written by
opys, not by you — but `history` is **required**, so a document you write by
hand needs one (see [The lifecycle map](#the-lifecycle-map-history)):

````markdown
---
id: FEAT-0421
status: implemented
tags: [osc, tabs, vte-parsing]
created: "2026-06-16T14:30:00Z"
updated: "2026-07-01T09:00:00Z"
history:
  "2026-06-16T14:30:00.000000Z": planned
  "2026-07-01T09:00:00.000000Z": implemented
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
### Setup
- external monitor at 150% scaling, default profile
### Procedure
1. Open a tab
2. `printf '\033]2;Ünïcödé tîtle\007'`
3. Switch to another tab and back
### Expectations
- the tab shows `Ünïcödé tîtle`, crisp and unclipped, active and inactive
- fractional scaling introduces no blur (a human call — no test asserts it)
````

## Frontmatter

Frontmatter is **standard YAML** between `---` fences, parsed by a real YAML
parser. Unlike earlier versions of this system — which restricted frontmatter
to flat `key: value` lines — custom fields may now use the full YAML feature
set: nested mappings, sequences, and multiline/block scalars (`|`, `>`).

The CLI's serializer still emits **canonical, minimal frontmatter**: the five
core fields first, in this order — `id`, `status`, `tags`, `created`, `updated`
— then every remaining key alphabetically, with flat scalars and scalar lists
rendered inline (`tags: [osc, tabs]`). Complex custom values are written as
block YAML under their key; `history` is the one exception, rendered by its own
block (keys always quoted, entries sorted by instant — see
[The lifecycle map](#the-lifecycle-map-history)) in its alphabetical slot. Hand
edits may use any valid YAML; running a write command re-canonicalizes scalar
fields and may reflow the formatting (not the meaning) of complex values — so a
key written in the wrong slot is moved to its canonical one by the next write.

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
| `status` | yes | one of the type's declared `statuses` — for the shipped `feature` type, `planned` \| `partial` \| `implemented` \| `wontfix` \| `archived` |
| `tags` | yes | non-empty list, open vocabulary; each tag is lowercase kebab-case, optionally a `key:value` pair (`area:parsing`, repeatable — same key may appear with several values) or a `key=value` pair (`priority=high`, single-valued — each `key=` at most once per document) |
| `created` | no | RFC3339 datetime (e.g. `2026-06-16T14:30:00Z`); set once at creation; auto-maintained |
| `updated` | no | RFC3339 datetime; refreshed on every user-initiated write; auto-maintained |
| `references` | no | ID→title map of linked work items (and features); auto-maintained |
| `blocked_by` / `blocks` | no | ID→title maps of the blocker relation; auto-maintained (see Blockers) |
| `history` | yes | the lifecycle map: RFC3339 **UTC** timestamp (six fractional digits, `Z`) → status, one quoted entry per line, sorted by key; auto-maintained — nothing hand-edits it (see The lifecycle map) |
| `<status>_reason` | per rule | one-line ADR for a status that demands one — the shipped config requires `wontfix_reason` at `wontfix` and `archived_reason` at `archived`, each through an ordinary `require_field` rule |
| `spec` | no | pointer to long-form shared material (a plain string field) |
| custom | per config | validated against `[fields.*]` declarations |

There is deliberately no `tests:` field — covering tests are derived from
the test plan, eliminating a sync surface.

The `references` map is **auto-maintained** by opys — it links a document to the
others it relates to and is kept bidirectional and title-fresh on every write.
You do not hand-edit it; a closed or retired document leaves a struck-through
(`~~title~~`) tombstone here (its id is also recorded in `_retired.md` — the
tombstone is presentation, the ledger is the reservation of record). Bare
`PREFIX-NNNN` ID mentions
in body prose are rewritten into markdown links on sync; text already inside
markdown link syntax (label or destination, including labels that themselves
contain `[…]` or inline code) is never re-linkified, and an existing link whose label starts
with a live ID has its title and path refreshed. A nested markdown link (a
complete link inside another link's label — invalid markdown, typically the
footprint of a bad edit) fails verify.

### The lifecycle map (`history`)

Every status a document has held, keyed by when it took it:

```yaml
history:
  "2026-06-16T14:30:00.000000Z": planned
  "2026-07-01T09:00:00.000000Z": partial
```

Keys are RFC3339 in **UTC**, always spelled with `Z` and exactly six fractional
digits. That spelling is required, not merely conventional: fixed-width keys are
the only ones whose lexical and chronological order agree, and the serializer
emits the block sorted, so `verify` rejects any other width (`2026-06-16T14:30:00Z`
included). Keys are quoted because a bare RFC3339 scalar is read as a
*timestamp*, not a string, by a YAML 1.1 reader — the corpus is read by more
than opys, so the quoting is part of the format and `verify` enforces it. A
write whose key is not strictly greater than the document's greatest existing
key is bumped by one microsecond, so two transitions inside the same second
cannot collide.

`new` writes the first entry (keyed to the same instant as `created`),
`set-status` appends — as does [`apply --status`](#composite-writes-opys-apply),
which is that same write inside a composite batch — and `import` seeds one entry
at the status it was given. `block`/`unblock` write no entry — they change no
status. **Nothing else writes it except the one-time migration seed below, and
nothing hand-edits it** —
`import` rejects a record that carries a `history` key, and the node's
`set-field` refuses it by name. A status change on
a document that predates the map seeds its first entry from `created`, at the
status being left, before appending — so a write never leaves a document whose
`created` and first key disagree. Unlike the relation maps, history cannot be
rebuilt from ground truth, so `sync` never repairs it — `verify` reports:

- a document with no history at all — absent, or a value carrying no entries
  whatever (`{}`, `[]`, a bare scalar): `missing history`;
- a value that is *there* and carries entries but is not a lifecycle map: a
  non-string key, an entry whose status is not a string, or a key that is not
  spelled canonically (RFC3339 UTC, `Z`, exactly six fractional digits). The
  message names the offending key or entry — or, when the value is not a mapping
  at all (a YAML list of entries, say), its shape;
- a last entry whose status is not the document's current `status`;
- keys that do not strictly increase;
- a `created` that is not the same second as the first key, when both exist;
- a consecutive pair that is not an edge the type's
  [status graph](#the-status-graph-typesnametransitions) declares, when it
  declares one. Two silences here are deliberate. A pair whose two statuses are
  **equal** is skipped, because `set-status <id> <its current status>` is a legal
  no-op that appends an entry and no successful write may leave a red corpus. And
  the **first** entry is never checked against `default_status`: `import` seeds a
  document at whatever status its record carried and leaves no marker saying so,
  so a rule of that shape could only be enforced by calling every imported
  document a problem. The graph constrains *moves*; where a document entered it
  is the seeding command's business;
- a key written **unquoted**. This one is checked against the file, not the
  parsed map: opys reads YAML 1.2, which resolves `2026-06-16T14:30:00.000000Z:
  planned` to the very same string→status pair as the quoted form, so every
  check above passes — while a YAML 1.1 reader sees a timestamp key and a
  document that is no longer a string→string map. Nothing repairs the spelling
  either: the next write re-quotes the block, so an unreported bare key would
  surface as a surprise diff long after it was written.

The shape rule is **all-or-nothing**, and it covers every YAML shape, not only
mappings: one bad pair makes the whole map unreadable, and so does a list or a
one-line string that carries entries. That is deliberate. A value opys cannot
read is data it must not destroy, so it is kept verbatim, reported, and left for
a human — and every write that would append to it (`set-status`, and
`apply --status` through it) **refuses** rather than replacing it. Only a value
holding no entries at all may be replaced by a seed. Reading only the
well-formed entries, or blessing a non-mapping as empty, would bless a map the
tool could not actually maintain, and the next status change would overwrite the
rest of it.

A document that has history but no `created` takes `created` from its first key
on the next write, rather than from the wall clock.

The map is also what makes `verify`'s two [time-aware
observations](#time-aware-observations-ttl-and-max_visits) possible: its last
key is when the document entered the status it holds, and the number of entries
recording a status is how often it has been there.

Seeding the map on an inventory that predates it is a one-time procedure, and
`opys history --seed <id>` is the tool for it: it reads the document's real
status timeline out of git (see `[git]` above) and writes one entry per status it
held, keyed by the commit that introduced it — UTC, six digits, quoted, like
every other entry. Three rules keep a seeded document green: the first entry is
keyed from `created` when the document has one (a document is never committed in
the second it was created, and the two must denote the same second); a key that
is not strictly greater than the previous one is bumped by a microsecond, so a
timeline predating `created` still increases; and a working copy whose `status`
is ahead of the last commit gets a final entry at the wall clock. It **refuses**
to overwrite anything carrying entries, and refuses when git has no committed
revision carrying a status — history is not rebuildable, so what is already there
is data. `docs/migration-history.md` is the full procedure, including the
`created` fallback for documents git cannot answer for.

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
`history`, `references`, `blocked_by`, `blocks`) are always allowed; every other key must be
declared under the type's
`[types.<name>.fields.*]` or verify rejects it. Richer YAML does not relax
the declare-or-fail rule.

### Filtering by field

`opys list` filters by `--tag` and `--status`, and additionally by any custom
field with repeatable `--field <key>=<value>` (ANDed together). A scalar field
matches on equality; a `list` field matches when it *contains* the value.
`--tag` matches an exact tag *or* any tag with that key (the head before `:` or
`=`), so a key search returns every value:

```sh
opys list --tag osc                             # exact tag, or key `osc`
opys list --tag area --format ids               # any area:* / area=* tag
opys list --field risk=high                     # enum/scalar equality
opys list --status partial --field area=cli     # combine with status
opys list --field tag-list=osc --format ids     # list-membership match
opys list --blocked                             # something live still blocks it
opys list --unblocked --format ids              # ready to pick up
opys list --sort --format ids                   # the type's order_by chain, not load order
```

`opys list --type <name>` restricts the listing to one document type.
`--blocked` / `--unblocked` filter on the derived blocked predicate (see
**Blockers**); they answer opposite questions, so passing both is a usage error.
`--sort` orders the listing by each type's `order_by` chain (see **Ordering**)
instead of load order.

`opys tags` enumerates the distinct tags in the inventory (sorted, one per line);
`opys tags --keys` collapses the `key:value` / `key=value` forms to their keys,
pairing with `list --tag <key>`. `opys stats` renders the project's configured
`[[stats]]` sections (see [`[[stats]]`](#stats--custom-stats-sections-optional));
the default config ships a per-type status breakdown, per-section coverage, and a
tag breakdown, but the report is fully configurable.

### Ordering (`order_by`) and `list --sort`

A type's `order_by` is the chain of tiebreaks its documents are put in
intentional order by. Four keys, each with a fixed direction:

| key | orders by | direction |
| --- | --- | --- |
| `priority` | **effective** priority (below) | ascending — lower number first, unprioritised **last** |
| `blocks` | how many live documents the item blocks | descending — unblock the most first |
| `id` | the numeric part of the id | ascending — oldest first |
| `updated` | the `updated` stamp | descending — newest first; missing last |

The default, for a type that declares no `order_by`, is
`["priority", "blocks", "id"]`. An `order_by = []` is the same as an absent one,
not an instruction to leave the listing unordered — the same absent-or-empty
reading `transitions` uses. Anything the chain cannot separate falls back to
the lowest id, so the order is total whatever the chain says. A key outside the
four is a **parse error** (exit 2), not a verify finding: the set is closed, and
a misspelled key would otherwise produce a plausible-looking wrong order.

**Effective priority** is the smallest `priority` over the document *and
everything it still blocks*, transitively. A task at `priority: 30` holding up
one at `priority: 1` sorts as a 1 — the work that unblocks the urgent thing is
as urgent as the thing. "Still blocks" is the live half of the `blocks` map (see
**Blockers**): not struck through, the target resolves, and the document itself
has not reached one of its type's `releases_blocks` statuses. A `blocks` cycle
terminates, and a document with no number of its own inherits whatever it holds
up. A `priority` that is not an integer reads as absent here; verify reports it
as a field-type problem.

`priority` is an **opt-in declared field** (`[types.<name>.fields.priority]`
with `type = "int"`, lower = higher) and the engine never demands it: on a type
that declares no such field the *implicit default* chain simply drops the
`priority` key, so the type sorts by blocks count, then id. Naming `priority` in
an **explicit** `order_by` on such a type is a config problem instead
(`config validate` / `verify` name the type), as is listing any key twice.

`opys list --sort` applies it. A mixed listing groups by type — the types in
the order `opys.toml` declares them, so a config leads with whichever type it
puts first — and orders each group by that type's chain; a document whose id
prefix matches no declared type sorts into a final group. With `--type` there is
one group, so it is simply the chain.
Without the flag `list` is in load order, unchanged. The chain is also what
[`opys next`](#the-pick-oracle-opys-next) picks with — the two read one ranking,
so they cannot disagree — and it is what the web UI's flow view lists each
status node with, through `next` itself, so the number it shows on an item is
the **effective** priority that ordered it, with the document's own beside it
when the two differ. The web UI board is the one place that orders a column by
the raw `priority` field instead, because that is the number drag-to-reorder
writes.

### The pick oracle (`opys next`)

`opys next --status <s> [--type <t>] [-n N] [--json] [--explain [ID]]` answers
one deterministic question: **given a status, which documents are pickable, best
first**. It is a read — it never syncs, never writes, and exits `0` for every
answer, an empty one included. It decides nothing about *who* picks the work up:
concurrency, occupancy, affinity and budgets are policy for whatever wraps it.

A document is **pickable** when all three hold:

- it is at the queried status (and of the queried type, when one is given);
- its type's `requires_link` is still satisfied — see **orphaned** below;
- nothing live blocks it (the derived predicate under **Blocked is derived**).

A blocked document is left out silently: a blocker is a normal, temporary state
of a healthy corpus, and it clears on its own. Pickability is evaluated on the
corpus the command runs in, which is **main**: a blocker closed on a branch
releases nothing until the merge lands, and `next` never opens git.

**Orphaned.** A document whose type declares `requires_link = { to, min }` is
orphaned when fewer than `min` of its `references` entries to that type still
*hold* — resolve to a document whose status its own type does not list in
`releases_blocks`. `requires_link` is a count rule, so one live target still
satisfies `min = 1` however many released ones are named beside it. An orphan is
**reported**, under its own `orphaned` key with the reason, rather than dropped:
it will not become pickable on its own, and the reason says which target failed
and how — released at a status its type declares, or resolving to no document at
all. `orphaned` is a diagnostic and is never truncated by `-n`. A document that
is both orphaned and blocked is reported once, as orphaned — the condition that
will not clear by itself.

Note what `releases_blocks` covers: it is where a type stops wanting work, and
the shipped `feature` type lists `implemented` there as well as `wontfix` and
`archived`. So a bug filed against a *shipped* feature is reported as orphaned
too, not only one under an abandoned feature — which is exactly why orphans are
listed with their reason instead of silently dropped. Re-link it, or reopen the
parent.

**Order** is the type's `order_by` chain over the whole corpus, grouped by type
in declaration order — the same ranking `list --sort` uses, so the two can never
put the same set in two orders. The default limit is **10**; `-n` truncates the
listing only, and every item carries its 1-based `position` within the full
pickable set, which `total` counts.

Each item also carries the [time observations](#time-aware-observations-ttl-and-max_visits)
its type configures, so a caller can see that the top item has been sitting
untouched. Reported only — `next` moves nothing, and it never exits `3`; that
code is `verify`'s.

A `--type` no configured type declares is **refused**, exit 2; so is a status —
measured against the named type's `statuses` when `--type` is given, and against
every configured type otherwise, so `--status planned --type task` is refused
where `planned` belongs to `feature` alone. An `--explain <ID>` naming no
document is refused the same way, as `show` refuses one. An oracle that answers
"nothing" to a typo cannot be told apart from an empty queue; an empty result for
a status that *exists* is a perfectly good answer.

`--explain` prints one line per listed document after the table, and
`--explain <ID>` explains that one — the five ways out, in the precedence they
are decided in (the type line can only appear under `--type`, since a document's
type is its id prefix):

```
TASK-0002: listed 1 of 3 — effective priority 1 (its own 30, inherited), blocks 1 live document
NOTE-0005: not listed — its type is note, not task
TASK-0009: not listed — it is at doing, not todo
TASK-0007: not listed — orphaned: FEAT-0006 is at wontfix, which releases what it blocks; 0/1 required feature links still hold
TASK-0008: not listed — blocked by TASK-0004
TASK-0003: not listed — pickable but ordered 2 of 3, behind the 1 listed (limit 1)
```

`--json` is the documented, stable payload — `schema: "opys.next/1"`, fields may
be added within that number:

```json
{
  "schema": "opys.next/1",
  "status": "todo",
  "type": "task",
  "limit": 1,
  "total": 2,
  "items": [
    {
      "id": "TASK-0004",
      "type": "task",
      "title": "Do this first",
      "status": "todo",
      "path": "opys/TASK-0004.md",
      "tags": [],
      "priority": 5,
      "effective_priority": 5,
      "blocks": 0,
      "position": 1,
      "observations": []
    }
  ],
  "orphaned": [
    {
      "id": "TASK-0003",
      "type": "task",
      "title": "Stranded",
      "status": "todo",
      "path": "opys/TASK-0003.md",
      "reason": "FEAT-0001 is at wontfix, which releases what it blocks; 0/1 required feature links still hold"
    }
  ]
}
```

`path` is relative to the project root. **Four fields are nullable**, and the
example above — a `--type task` query over prioritised documents — happens to
populate all of them: the top-level `type` is `null` unless `--type` was given;
an item's `type` is `null` when its id prefix matches no configured type; and
`priority` and `effective_priority` are `null` when the document declares no
integer `priority` and nothing in its `blocks` closure does — the ordinary case,
since priority is opt-in. A consumer that types those as required numbers or
strings breaks on the first unprioritised document. Everything else is always
present: `tags` and `observations` are arrays (empty, never absent), and
`explain` is the one *omitted* key — it appears only under `--explain <ID>`. So
the common default-invocation shape is:

```json
{
  "schema": "opys.next/1",
  "status": "todo",
  "type": null,
  "limit": 10,
  "total": 1,
  "items": [
    {
      "id": "TASK-0004",
      "type": "task",
      "title": "Do this first",
      "status": "todo",
      "path": "opys/TASK-0004.md",
      "tags": [],
      "priority": null,
      "effective_priority": null,
      "blocks": 0,
      "position": 1,
      "observations": []
    }
  ],
  "orphaned": []
}
```

An `observations` entry is one flat object — the kind's own fields sit beside a
`"kind"` tag, with no wrapper to unwrap, and an overflow is listed before a stuck
on the same document:

```json
      "observations": [
        {
          "id": "TASK-0006",
          "kind": "overflow",
          "status": "todo",
          "visits": 2,
          "limit": 2,
          "target": "doing"
        },
        {
          "id": "TASK-0006",
          "kind": "stuck",
          "status": "todo",
          "since": "2026-01-02T03:04:05.000000Z",
          "after": "10m",
          "target": null
        }
      ]
```

With `--explain` and no ID the JSON is unchanged (the items already carry
`priority`, `effective_priority`, `blocks`, `position` and `total`);
`--explain <ID>` adds one top-level `explain` object tagged by `reason`
(`listed`, `other-type`, `other-status`, `orphaned`, `blocked`, `behind`),
carrying the same `message` the human output prints.

The node serves the same payload from the same function at
`GET /api/projects/{key}/next?status=&type=&n=`, where `{key}` is a project key
from `/api/projects`. A project is a repository and its worktrees, and the answer
comes from its **main** worktree, because main is the truth for pickability.
Its sibling `GET /api/projects/{key}/flow?n=` draws the whole lifecycle — one
node per distinct status, edges from the union of every type's `transitions`,
and each node's top `n` documents (default 5) taken from this same function, so
the drawing and the oracle cannot disagree.

## Blockers

Mark a dependency between two items (features and/or work items) with
`opys block <id> --by <blocker-id>`; `opys unblock <id> --by <blocker-id>`
removes it. The relation is **directional and bidirectional**: the blocked item
gains a `blocked_by` entry and the blocker gains the inverse `blocks` entry,
both kept title-fresh and sorted automatically (you do not hand-edit them).
Either id may be a document of any type. The blocked side takes an id list like
every other mutating command (see **Bulk edits** below), so
`opys block FEAT-0001 FEAT-0002 --by FEAT-0009` points several documents at one blocker
in a single command.

Blocker entries resolve, tombstone on close (`TASK-0042: ~~title~~`), and reserve
ids exactly like `references`; a closed blocker is therefore safe to leave in
place, and `opys cleanup` strips the struck entries (ledger-reserving any id
whose last tombstone it strips).

### Blocked is derived

There is no `blocked` status, no `blocked_from` and no `blocked_reason`. A
document is blocked **iff it holds a live `blocked_by` entry**, and an entry is
live when all three of these hold:

- it is not struck through (`~~title~~` is a closed blocker's tombstone);
- it resolves to a document that exists;
- that document's status is not listed in its own type's `releases_blocks`.

So blocking and unblocking are relation writes and nothing else: `block` and
`unblock` never touch `status` and never append to `history`. A blocker stops
blocking by *moving*, without anything being written to the document it blocks.

`releases_blocks` is the per-type list of statuses at which a document of that
type lets go of whatever it blocks:

```toml
[types.feature]
releases_blocks = ["implemented", "wontfix", "archived"]
```

It earns its keep on a **permanent** type — one that is never closed, and would
otherwise block forever. On an **ephemeral** type it is accepted but redundant:
`close` deletes the document and strikes every entry pointing at it, so such a
type releases by construction. Absent (the default) a document of the type
blocks at every status it can hold. A status in `releases_blocks` that the type
does not declare is a config problem `verify` and `config validate` report.

Everything that blocks is a *document*: "waiting on a vendor" is a chore, not a
free-text reason. `opys list --blocked` / `--unblocked` filter on the predicate,
and the node's board badge and document view read the same function.

## Status semantics

- `planned` — in inventory, no implementation.
- `partial` — some behavior present; unchecked test-plan items document the gap.
- `implemented` — complete; requires ≥1 checked test-plan item.
- `wontfix` — deliberate exception, reason required; stays in the inventory so
  the decision is recorded and not re-litigated.
- `archived` — removed from the product, reason required. "Removed" is a status
  and never a deletion: the record stays, and the shipped `status_dirs` moves
  the file under `_archived/`.

Where a document **starts** is not a choice: `new` has no `--status`, so a
document takes its type's `default_status` and the graph below is always
entered at a known point (which is also why a document's first `history` entry
always agrees with its `created`). `import` is the one seam that seeds a
document anywhere in the lifecycle, and `verify` knows it — see
[Bulk creation and migration](#bulk-creation-and-migration). A **terminal**
status is the mirror image: it is reached only through `close`, which deletes
the document.

Status changes go through `set-status` — or through
[`apply --status`](#composite-writes-opys-apply), which performs the same write
as one part of a composite batch — never hand edits: the guards live there, and
each one is recorded in the document's `history` map. Both take an id list, so a
whole selection moves in one command (see **Bulk edits**).
A `log` section is the same kind of thing: it is written by `opys log`, never by
hand (see [Log sections](#log-sections)).

### The status graph (`[types.<name>.transitions]`)

A type may declare which status may follow which:

```toml
[types.task.transitions]
todo        = ["in-progress"]
in-progress = ["review", "todo"]
review      = ["done", "in-progress", "user-review"]
user-review = ["done", "in-progress"]
```

Omit the table and the type keeps the **any-to-any** rule: every status may
follow every other, which is what every config predating the graph does, and is
still a perfectly good answer for a type whose statuses are labels rather than
stages. Declare it and it is the whole truth:

- a status that is **not a key** has no outgoing edges at all — a dead end, which
  is often the point (`superseded` is where an ADR stops). Half a map does not
  mean "these two moves are constrained and the rest are free";
- a **terminal** status may not be a key. It is reached only through `close`,
  which deletes the document, so there is nothing left to move on. An edge
  *into* one (`review = ["done"]`) is meaningful and is exactly what says
  "a task may be closed from review";
- the **initial** status (`default_status`) must have at least one outgoing
  edge, or every document of the type is stuck the moment `new` creates it. A
  key with an empty list (`todo = []`) strands the corpus exactly as a missing
  key does, and is reported the same way;
- every key and every target must be a status the type declares. Duplicated
  targets are deduplicated silently; declared order is preserved, and is the
  order messages and the node's status menu use.

Those four are `verify` / `config validate` problems (`opys.toml: type '…': …`,
exit 1), reported like every other config problem rather than raised as a hard
error — a typo in the graph must not change the exit code of every command.

Four places enforce the graph:

- **`set-status`** refuses a move the type does not declare, exit 2, naming
  where the document stands and where it may go (`cannot move TASK-0007 from
  todo to review: type 'task' declares no such transition (from todo:
  in-progress)`). What it names is what *that* command can reach, so a terminal
  target is left out of the list and pointed at its own command instead — from
  `review`, under the graph above, the refusal ends
  ``(from review: in-progress, user-review; `opys close` reaches done)``.
  Setting a document to the status it already holds is *not* an
  edge: it stays the legal no-op it has always been, and appends an entry —
  unless the type's `max_visits` budget for that status is spent, since an entry
  is a visit (see [Time-aware observations](#time-aware-observations-ttl-and-max_visits)).
- **`apply --status`** refuses the same undeclared moves, and one more: the
  equal case. A composite write *asserts a transition*, so a document that
  already holds the status being asked for is refused rather than written to a
  second time — whether or not the type declares a map. That is what makes a
  claim race decidable; see
  [Composite writes](#composite-writes-opys-apply).
- **`close`** refuses unless the current status has an edge into one of the
  type's terminal statuses, naming the statuses that can close. `--force` (about
  unchecked checklist items) does not bypass it.
- **`verify`** checks every consecutive pair of the `history` map against the
  graph, so a hand edit, an older opys, or a graph tightened after the fact is a
  CI finding rather than an invisible snapshot. See
  [The lifecycle map](#the-lifecycle-map-history) for the two things it
  deliberately does *not* check.

Tightening a graph is retroactive, because history is an ongoing record and is
never repaired automatically: pairs already written that the new graph does not
declare become `verify` findings. Keep the old edge declared, or repair the
affected documents by hand (`docs/migration-history.md`).

### Time-aware observations (`ttl` and `max_visits`)

Two things worth noticing are not content problems. A document can sit in a
status past any reasonable time for the project — whoever took it is gone — and
a document can bounce between two statuses forever, because the coder and the
reviewer disagree and neither is a person. Neither is a snapshot invariant: a
corpus is valid when written and *becomes* stuck with no write at all. So they
are **observations**: reported apart from problems, with their own exit code,
and never mixed into the problem list.

Both thresholds are per-type, per-status config — a fast repository wants
minutes where a "ship the game" feature may legitimately sit for two years:

```toml
[types.task.ttl]
review      = "2d"                               # observed only, nothing to fix
in-progress = { after = "10m", then = "stuck" }  # `verify --fix` performs this

[types.task.max_visits]
review = { limit = 3, then = "user-review" }

[types.task.transitions]
todo        = ["in-progress"]
in-progress = ["review", "stuck", "user-review"]   # …and the escalation edge
stuck       = ["in-progress", "todo"]   # the recovery edge a fix relies on
review      = ["in-progress", "user-review", "done"]
user-review = ["review", "in-progress"]
```

A **duration** is one or more `<integer><unit>` groups, units `s`, `m`, `h`,
`d`, `w` (7 days), `y` (365 days): `10m`, `2d`, `1h30m`, `2y`. Nothing else — no
spaces, no sign, no decimal point, no bare number.

- **Stuck** — the document's last history key is more than `after` in the past.
  A `ttl` written as a bare duration is reported and nothing more; the
  `{ after, then }` form names where `--fix` moves it.
- **Overflowed** — the document's history records it entering the status it
  currently holds `limit` times. `limit = N` *allows* N entries: a document that
  has made its N-th is at its limit, so `verify` reports it and `set-status`
  refuses another entry, naming `then` (`cannot set TASK-0007 to review: type
  'task' allows 3 visit(s) to review and this document has had 3 — max_visits
  sends it to user-review instead`). The guard and the observation deliberately
  trip at the same count: a refusal with no observation saying where the
  document should go instead would be a dead end rather than an escape hatch.

They trip at the same count but not in the same *place*, and the difference is
what the config checks below are for. The **observation** counts the status the
document holds *now* — that is the one a fix moves it out of. The **guard**
counts the status being entered, so it refuses the move from wherever the
document stands; a document that has already bounced back out of the budgeted
status is not observed (there is nothing to fix: it is not there) but still
cannot return.

`config validate` (and so `verify`) checks each entry, exit 1 like every other
config problem: every key and every `then` is a declared status; `then` is not
the key itself and not terminal (a terminal status is reached only via `close`,
which `set-status` refuses, so the fix could never run); `limit` is at least 1;
the duration parses. Where the type also declares a **status graph**, the fix
edge `key -> then` must exist — the fix is an ordinary `set-status` — and so
must the **recovery edge** `then -> key`, because a fix may land under a running
agent and that agent must be one `set-status` away from where it was. A
`max_visits` budget is checked once more: every status with an edge *into* the
budgeted one must declare an edge to `then` as well — the **escalation edge**,
because that is where the guard's refusal sends a document standing there, and
advice that is not a legal move is a trap. (`then` itself is exempt: a document
already there needs no move.) A type with no graph is exempt from all three: it
allows every move already.

One consequence worth stating: once a document has spent its `max_visits`
budget it cannot re-enter that status even along a declared edge. `then` is
where a person (or a different agent) takes over.

A visit is a `history` entry, and in ordinary use only `set-status` and `apply
--status` append one to a document that already exists — plus the one-time
`opys history --seed` migration (see [The lifecycle
map](#the-lifecycle-map-history)), whose reconstructed entries count as visits
like any other, so seeding a map from git can leave a document at or over a
budget no `set-status` ever spent. So a relation write is never a visit:
`block` and `unblock` change no status and write no entry (see
[Blocked is derived](#blocked-is-derived)), and a document they touch neither
spends its budget nor escapes one it has already spent.

### `verify` exit codes and `--fix`

| Code | Meaning |
|---|---|
| `0` | clean |
| `1` | content problems (with or without observations) |
| `2` | a real failure: bad flags, IO, a missing or unreadable config |
| `3` | observations, and nothing else |

Problems and observations are two blocks on **stderr**, problems first:

```
verify: 1 problem(s)
  FEAT-0002: missing required '## Test plan' section
verify: 2 observation(s)
  TASK-0001: stuck in in-progress since 2026-01-02T03:04:05.000001Z (ttl 10m) — `--fix` moves it to stuck
  TASK-0002: 3 visits to review reaches its limit of 3 — `--fix` moves it to user-review
```

`verify: OK (N documents)` is printed on stdout for exit `0` only — a nonzero
run never claims to be OK. **Gate CI on `1`**: most scripts gate on any nonzero
code, which trips on `3` as well, and a corpus that only has observations was
clean when it was written.

`verify --fix` performs the fixable observations first: each is an ordinary
transition (history appended, the layout relocation performed, the sync pass run
— skipped, like every other mutating command's, under `--no-sync`). One line per
fix goes to **stdout**, before the re-verify's blocks:

```
fixed TASK-0001: in-progress -> stuck (stuck)
fixed TASK-0002: review -> user-review (max_visits)
```

The exit code then reports what *remains* — including any observe-only `ttl`
entry, whose observation line ends `— no ttl target, nothing to fix` and which is
exit `3` again. Five rules make it safe to run unattended:

- at most **one fix per document per run**: a document can be both stuck and
  overflowed, and the second move would start from a status the first one just
  left. The overflow goes first;
- a **refusal** — a `then` the engine will not move the document to, whether
  because the status has a `[[rules]]` guard the document cannot satisfy or
  because its own visit budget is spent — is reported as a problem
  (`TASK-0001: could not fix (stuck): …`) and the pass continues, so one bad
  document never turns `verify` into exit `2`;
- **nothing is written at all when there was nothing to fix**, so a clean run is
  never a silent `sync` over hand-written bytes;
- **every fix is subject to the visit budget**, the overflow fix included: the
  guard counts entries into the status being *entered*, so exempting a fix could
  only ever waive the budget of the target it moves *to* — writing the very
  count `limit` is defined to be the last of, and performing a move `set-status`
  refuses by name. A target whose own budget is spent lands in the refusals
  instead, naming *its* `then` (the next step up an escalation chain), which
  also means a `verify --fix` loop terminates;
- and `--fix` **stands down entirely while the config has problems** — it
  performs transitions along the graph `config validate` checks, so fixing under
  a config that check rejects is how a document ends up parked in a dead end. It
  says so once on stderr (`verify: --fix skipped — opys.toml has problems, …`)
  and reports the observations unfixed.

The clock is injectable: **`OPYS_NOW`** (RFC3339) overrides it for every
timestamp opys writes and every observation it measures, so tests and repeated
CI runs of one commit are deterministic.

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

## Log sections

A section declared `kind = "log"` is a **dated timeline**: a flat list of
top-level `- ` bullets, each stamped with the UTC time it was written.

```markdown
## Progress
- 2026-09-05T14:03:22Z picked it up; the flock is the part that needs care
  a lease file can go stale, a flock cannot — the OS releases it with
  the process
- 2026-09-05T18:41:07Z landed, gate green
```

- The stamp is **RFC3339 UTC at whole seconds, `Z`** — the same clock as the
  `history` map, one precision coarser, because a log is read by people.
- A bullet's continuation lines are indented two spaces under it and belong to
  that entry. Anything that is not a top-level `- ` bullet is continuation.

### `opys log`

Entries go in through the command, never by hand:

```sh
opys log TASK-0042 "chose the flock; a lease file can go stale"
opys log TASK-0042 - <<'EOF'            # `-` reads the entry from stdin
chose the flock
rejected: a lock directory, which a crash leaves behind
EOF
opys log NOTE-0007 "…" --section Decisions   # only when the type has >1
```

It resolves the document type's `kind = "log"` sections: none is an error (exit
2, naming the type), more than one needs `--section`, and a section the type
declares but *this document* lacks is created at the end of the body. Then it
stamps the entry, splices it into the section without moving a byte outside it,
bumps `updated`, and runs the ordinary sync pass — so a bare `FEAT-0001` in the
entry is linkified like any other prose.

`log` is the one mutating command that takes a **single** id. The second
positional is the entry text and `-` there already means "read it from stdin",
so an id list would be ambiguous about which of the two the pipe is feeding
(contrast **Bulk edits**).

### What verify checks

1. **Every bullet is dated.** An undated one is reported by name and by its
   opening words. Existing inventories therefore report their old bullets until
   they are migrated — see `docs/migration-history.md`.
2. **Entries do not go backwards.** The check is *non-decreasing*, not strictly
   increasing: two entries may share a stamp (two `opys log` calls in one
   second, or a migration dating a document's bullets from its `created`). Only
   a stamp *earlier* than the one above it is a problem. (The `history` map is
   strict, for a different reason: its keys are also its sort order.)

### `require_log_since_status`

The rule assertion that makes a log *required at a transition*:

```toml
[[rules]]
when = { type = "task", status = "review" }
require_log_since_status = "Progress"
```

It passes when the named log section holds an entry stamped at or after
**`since`**, compared at seconds granularity. `since` is the timestamp of the
**last `history` entry whose status differs** from the status being evaluated,
falling back to the first key when every entry holds it. A document with no
`history` at all passes (verify reports that separately).

That definition, rather than "the last history key", is what makes the
write-time and the verify-time answer agree. `set-status` evaluates the rules
*before* appending the new entry, so at write time the map ends at the status
being left; at verify time the new entry is there and "last differing" skips it.
Worked example — a task moving to `review`:

| history | evaluated at | `since` |
| --- | --- | --- |
| `todo@T0, in-progress@T1` | write time (`set-status … review`) | `T1` |
| `todo@T0, in-progress@T1, review@T2` | verify time | `T1` |
| `…, review@T2, in-progress@T3` | the bounce back into `review` | `T3` |

So the entry that satisfied the first visit to `review` is stale for the second:
each bounce needs its own checkpoint. The shipped config puts this on the move
into `review` for `task`, `bug` and `chore`, so a handoff without a checkpoint
is refused.

The checkpoint and the transition still fit in one invocation:
`opys apply … --status review --log "…"` writes the entry before it evaluates
the rule, so the rule is satisfied by the very command it guards (see
[Composite writes](#composite-writes-opys-apply)).

## Structured sections

A `structured` section's content shape is **configured**, not hardcoded: the
section declares a `structure` — an [`mdprism`](../../../docs/structure-dsl-spec.md)
schema (the body portion of the DSL). `verify` runs `mdprism` against the
section's content; `new` scaffolds the section from the same schema.

```toml
[[types.feature.sections]]
heading = "Manual verification"
kind = "structured"
structure = '''
### @setup Setup
  - +@items
### @procedure Procedure
  1. +@steps
### @expect Expectations
  - +@checks
'''
```

That structure requires the page to look like:

```markdown
## Manual verification
### Setup
- external monitor at 150% scaling
### Procedure
1. Open a tab
2. `printf '\033]2;Ünïcödé\007'`
### Expectations
- crisp glyphs, no clipping
```

The schema is whatever a type needs — sub-sections, typed/nested lists,
checklists, regex-constrained items, cardinality, capture aliases — see the DSL
spec and [reference](../../../docs/mdprism-reference.md). The default config
ships the manual-QA shape above; rename, restructure, or drop it per type.

Conventions:

- Write for a competent operator who knows the project but not this case: spell
  out exact escape sequences, config values, and the precise defect to look for.
- Procedures longer than ~10 lines or shared across features move to a shared
  doc and are referenced.

(The default coverage stat counts every present section with countable items —
`checklist`, `log`, and `structured` — but only `checklist` sections report an
`uncovered` count, since only they have unchecked items.)

## Bulk creation and migration

`opys new` creates one feature per process and runs a full sync each time
— fine interactively, far too slow for migrating hundreds or thousands of
features. Two supported bulk paths avoid that:

- **`opys import <file.jsonl>`** — one JSON object per line, each describing a
  feature. `title` and `tags` are required; `status` (default `planned`),
  `spec`, and any declared custom fields are optional top-level keys; `body` is
  optional markdown placed under the `# Title` heading (use it to carry a
  `## Test plan` or `## Manual verification`). The auto-allocated `id` and the
  auto-maintained `history` are rejected by name — import allocates the one and
  seeds the other with a single entry at the record's status. One ID allocation and one sync
  cover the whole batch, and it is **transactional** — if any record is
  rejected, nothing is written. The record's status is checked against the
  type's statuses and its `[[rules]]` (a record with `"status": "implemented"`
  must include a checked test-plan item in its `body`), but **not** against the
  type's status graph: import is the one seam that seeds a document anywhere in
  the lifecycle, and `verify` knows it (see the lifecycle map). Run `opys verify` afterwards for the deep
  checks (tag shape, reference resolution, field types). Example line:

  ```json
  {"title": "Tab title follows OSC 0/2", "tags": ["osc", "tabs"], "status": "implemented", "ptyxis_ref": "src/ptyxis-tab.c", "body": "## Test plan\n- [x] OSC 2 updates title — `tab::osc_title`"}
  ```

- **Write the files directly** — `opys` reads plain markdown, so you can emit
  canonical `FEAT-NNNN.md` files yourself (matching the frontmatter rules
  above), then run `opys sync` once and `opys verify`. This is a fully
  supported escape hatch when your source data does not map cleanly onto the
  JSONL schema; allocate IDs monotonically and never reuse a retired one (and,
  under an `[ids] modulus`, stay in your slot — see
  [Id allocation](#id-allocation)).

Either way: do **not** loop `opys new` over a large migration. After import,
review in batches per tag using `opys list`, exactly as for a hand-built
inventory.

## SQL queries (`opys query`)

`opys query` is **read-only**. Every statement must be a SELECT — checked on the
plan before anything executes, so a compound like `SELECT 1; DELETE FROM docs`
is rejected whole — and the command never writes files. All tables are
queryable: the authoritative `docs`/`tags`/`relations`/`history`/`fm_fields`/`retired`,
and the derived `fields`/`sections`/`blocks` projections (`blocks` decomposes
each body into one row per `##` section, so you can query into prose). `-` reads
the SQL itself from stdin; `--stdin` binds stdin to `$1` for a large or
multi-line value that would otherwise need SQL escaping.

There is no write mode. Every change goes through a lifecycle command, so it is
guarded by the rules and leaves a valid document — SQL cannot bypass that.

## Bulk edits

Every id-taking mutating command (`set-status`, `tag`, `retire`, `block`,
`unblock`, `close`, `apply`) takes a **list** of ids, three interchangeable
ways: several positionals (`opys set-status FEAT-0001 FEAT-0002 partial`), a
comma-separated list (`FEAT-0001,FEAT-0002`), or the literal `-`, which reads
the list from stdin with comma, space and newline separation all accepted. So a
query and an edit compose:

```sh
opys list --type task --status review --format ids | opys close -
opys list --tag legacy --format ids | opys tag - --add deprecated
```

A batch is **all-or-nothing**: ids are applied in order against one loaded
corpus, and the first failure (an unknown id, a refused transition, a rule
violation) aborts the whole invocation with exit code 2 and writes nothing —
not even the ids that had already succeeded. Repeated ids are collapsed, keeping
first-seen order.

`opys log` is the exception: it takes one id, because its second positional is
the entry text and `-` there means stdin (see [Log sections](#log-sections)).

Select on a status a live document can actually hold. A terminal status is only
ever reached *through* `close`, which deletes the document, so
`--status done | opys close -` selects nothing, and an empty selection is a
failure like any other: `-` with nothing on stdin exits 2 (`expected at least
one id`) and writes nothing. A pipeline that may legitimately match nothing has
to guard its producer. In the other direction, a producer whose reader goes away
(`opys list --format ids | head -1`) stops quietly at exit 0 — no command turns
a closed pipe into a crash.

So one document that a rule refuses stops the batch its list belongs to: a
`close` list holding a document with an unchecked required checklist item closes
none of them. Narrow the list — or fix that document — rather than reaching for
`close --force`, which drops the checklist guard for every id in the list.

## Composite writes (`opys apply`)

**Bulk edits** are all-or-nothing along the *id* axis. `opys apply` is
all-or-nothing along the *action* axis: a status transition, one or more field
writes, and a log entry, named in one invocation, applied against one loaded
corpus and flushed once or not at all.

```sh
# claim a document: take it, record who took it, say so — atomically
opys apply TASK-0007 --status in-progress --field claimed_by=agent-7 --log "picked up"

# release it again: the same shape, backwards
opys apply TASK-0007 --status todo --remove-field claimed_by --log "handing back"
```

The two axes compose. `apply` takes an id list like every other mutating
command, and a refusal anywhere — any part, any id — abandons the whole
invocation with exit 2 and leaves every document byte-identical.

### The parts are applied in a fixed order

**`--field` writes, then the log entry, then the status, then `--remove-field`
removals** — and the order is not the caller's to change. The two steps before
the status are there because the status step reads them; the removals are last
because nothing reads them at all.

The status step evaluates the type's rules against the document as it stands in
the corpus at that moment, so both steps before it are already visible:

- a `require_field` rule on the target status sees the field this very
  invocation wrote;
- a [`require_log_since_status`](#require_log_since_status) rule sees the entry
  this very invocation appended — so a move into a status guarded by it is legal
  in one command, where a bare `set-status` is refused for want of a checkpoint.

**The verify gate is taken once around the whole batch**, not once per field.
The corpus's findings are recorded before the first step and compared after the
last, so every part of the batch is judged against the document the batch
*finishes* with — never against a half-applied one at a status the invocation is
on its way out of. Take the natural claim config, which is what makes a claim
mean anything at all:

```toml
[[rules]]
when = { type = "task", status = "doing" }
require_field = "claimed_by"

[[rules]]
when = { type = "task", status = "doing" }
field_matches = { field = "claimed_by", pattern = "^agent-" }
```

Both halves of a hand-off stay one command under it:

- releasing — `--status todo --remove-field claimed_by` — is not refused by the
  `doing`-only `require_field` rule the document is leaving;
- handing on — `--status review --field claimed_by=reviewer-3` — is not refused
  by the `doing`-only `field_matches` rule, which the finished document is no
  longer subject to.

A write or a removal the **target** status forbids is still refused by name: the
target status is the one the finished document holds. Only the status being left
loses its say, and only over the parts of the batch that leave with it.

### The race it settles

Two agents read the same document as pickable and both try to take it. Exactly
one may win, and the loser must write nothing at all: not a partial claim, not a
log line, not a field.

What decides which one wins is the **state machine, not the lock**. `--status`
here asserts a *transition*: the document must not already hold the status being
asked for. The inventory lock only serializes the two invocations; whichever
takes it second loads a corpus in which the winner's claim has already landed,
finds the document in the working status, and is refused before it has written
anything:

```
cannot move TASK-0007 from in-progress to in-progress: it is already
in-progress, and a composite write asserts a transition — another writer got
there first
```

That refusal is the one place `apply` and `set-status` differ, and deliberately
so: `set-status TASK-0007 in-progress` on a document already there is the legal
no-op it has always been, while a claim that silently overwrote the previous
claimant would be no claim at all. It holds whether or not the type declares a
`[transitions]` map — a graph-less type still cannot be claimed twice — and
where a map is declared, an ordinary undeclared edge is refused with the graph's
own message. A refusal for the lock (`… waiting for the inventory lock …`) says
so in its own words, and means the write never ran at all rather than that it
lost.

### The claimant is an ordinary declared field

opys has no notion of an agent. The field that records who took a document is
one the project declares, like any other:

```toml
[types.task.fields.claimed_by]
type = "string"
```

It filters with `list --field claimed_by=agent-7` and appears in the SQL
`fields` table, and that is the whole of opys' involvement: it stores who
claimed a document and knows nothing else about them. Leases, expiry, agent
identity, and the policy of *which* document to claim belong to whatever
dispatches the work, not to the tracker.

### Grammar notes

- Every part is optional, but an invocation that asks for nothing at all is a
  usage error rather than a write that only bumps `updated`.
- `--reason` writes `<status>_reason`, exactly as it does for `set-status`, and
  so requires `--status`. `--section` names the section `--log` appends to, and
  so requires `--log`. Each is read by exactly one step, and passing one without
  that step is a usage error rather than a flag that silently does nothing —
  `--section` in particular is checked against the type's sections, so a
  misspelling must not depend on which other flags happen to be present.
- `--field key=value` and `--remove-field key` are repeatable and go through the
  same verify gate the node's edit-in-place uses: an undeclared key, a value
  that does not fit the declared type, a broken `enum` or `pattern` — each is
  refused by name and abandons the invocation.
- `--log -` does **not** read stdin. `-` already means "read the id list from
  stdin", and one invocation cannot have two stdin consumers; pass the entry as
  a single argument, or use `opys log <id> -` for the single-document case.
- The node exposes the same composite as one typed `apply` action, so a
  dispatcher over HTTP gets exactly the atomicity a dispatcher over the CLI
  gets — including these usage errors, which the engine raises rather than the
  CLI parser: a body naming no part, or a `reason`/`section` with no
  `status`/`log`, comes back 422 with the words above.

## What never goes in feature files

Test results, execution dates, completion claims, assignees, or sprint
metadata. CI owns automated results; this system owns intent only. (Priority
is the one carve-out: a project may declare a `priority` int *field* —
lower = higher — and tools order by it through a type's `order_by` chain (see
**Ordering**). It is frontmatter, never prose.
The auto-maintained `created`/`updated`/`history` timestamps are not an
exception either: they record when opys wrote, not when work happened, and so
do a `log` section's stamps, which `opys log` writes and nobody types.)

A *work item* — never a feature — may also declare a field recording who
currently holds it (`claimed_by`), written atomically with the transition that
takes it (see [Composite writes](#composite-writes-opys-apply)). That is a
lock, not an assignment: it says who is working on a throwaway document right
now, and it goes away with the document.

Implementation logs, task checklists, and branch/PR links also do not belong
here — they go in an ephemeral document (a `task`/`bug`/`chore`), which is
deleted when the change lands. The feature is permanent; the work item is
throwaway.
