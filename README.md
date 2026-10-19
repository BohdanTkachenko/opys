# opys

File-based inventory of typed markdown documents for human + AI codebases — one
markdown file per document, verified in CI.

`opys` manages a version-controlled inventory of *what a product does*: one
markdown file per document, each with YAML frontmatter (stable ID, status,
tags) and an optional body (spec prose, a test plan, manual-verification
procedures). The document **types** — their ID prefixes, statuses, fields,
required sections, and validation rules — are configured in one
`opys.toml`. The default config ships a permanent **feature** type
(`FEAT-NNNN`) plus ephemeral **task/bug/chore** types (`TASK-`/`BUG-`/`CHORE-NNNN`)
for in-flight work, deleted on `close`. Writes go through the CLI so invariants
hold at write time and parallel agents don't collide; reads are plain `grep` +
targeted file reads. A `verify` subcommand is the CI gate. It is deliberately
*not* a task board — no sprints; ownership only as opt-in config on statuses.
Priority exists only as an opt-in declared int field (`[types.X.fields.priority]`) that the web UI's
board orders and reorders by, and that a type's `order_by` chain — what
`opys list --sort` applies — leads with.

Need a different lifecycle — an `epic`, an `adr`, a `risk`? Add a `[types.<name>]`
block to `opys.toml` and the whole tool (create, verify, query) works for
it. Durable knowledge → features; "what I'm doing right now" → a task/bug/chore.

Orchestration is not opys' job. Driving agents around this inventory — who
picks up what, when a stalled document is really abandoned — belongs to an
orchestrator that uses opys as its tracker. What is here is
the tracker: it records, guards and reports, and it starts no write off its own
clock.

It pairs with the `opys` skill (under `skills/`), which
documents the format and the authoring/implementation workflows for coding
agents.

## Install

```sh
cargo install opys                 # the CLI (what agents use)
```

Or build from source:

```sh
cargo build --release -p opys        # target/release/opys
```

### Use from another flake

The flake exposes `opys` as a package, an app, and an overlay, so other flakes
can consume the CLI without going through crates.io:

```nix
{
  inputs.opys.url = "github:BohdanTkachenko/opys";

  outputs = { nixpkgs, opys, ... }:
    let
      system = "x86_64-linux";
      # Either apply the overlay and use `pkgs.opys`…
      pkgs = import nixpkgs {
        inherit system;
        overlays = [ opys.overlays.default ];
      };
    in {
      devShells.${system}.default = pkgs.mkShell {
        # …or reference the package directly: opys.packages.${system}.default
        packages = [ pkgs.opys ];
      };
    };
}
```

Or run it straight from the flake, no install:

```sh
nix run github:BohdanTkachenko/opys -- --help
```

`opys.toml` lives at the **project root** — opys finds it by searching upward
from the current directory (like git or Cargo). It declares a `base` directory
(default `opys/`, relative to the root) so the inventory stays out of the
repo root: the document files, flat at `opys/` by default (the path is rendered
from a configurable `[layout]` template — see the spec). A document's type is its
ID prefix.

## Quick start

```sh
opys init                                   # bootstrap opys.toml + opys/
# edit opys.toml: types, statuses, transitions, fields, sections, rules

opys new --title "Tab title follows OSC 0/2" --tags osc,tabs
opys list --status planned
opys list --tag area                        # exact tag, or any tag with key `area`
opys list --sort --format ids               # the type's order_by chain: effective
                                            # priority, then blocks count, then id
# `implemented` is a declared edge, but a rule also wants a checked `## Test
# plan` item — and the shipped section check makes a checked item cite a test
# that actually exists under `roots`, so a test plan cannot drift from the suite:
mkdir -p src && printf 'fn title_survives_a_profile_switch() {}\n' > src/lib.rs
printf '\n## Test plan\n- [x] survives a profile switch — `lib::title_survives_a_profile_switch`\n' >> opys/FEAT-0001.md
opys set-status FEAT-0001 implemented
opys verify                                 # integrity check; exit 1 on problems, 3 on
                                            # time observations (`--fix` performs those)
opys stats                                  # configurable [[stats]] sections (default: status/coverage/tags)
opys tags                                   # distinct tags (--keys for just keys)

# Ephemeral work, linked to a feature (default types: task/bug/chore):
opys new --type bug --title "Survive profile switch" --features FEAT-0001
opys set-status BUG-0002 in-progress        # the shipped work-item graph runs
opys log BUG-0002 "reproduced: the title survives, the icon does not"
opys set-status BUG-0002 review             # todo -> in-progress -> review, and the
                                            # move into review wants a `## Progress`
                                            # entry since the status last changed
sed -i 's/- \[ \] First item/- [x] fixed in the OSC parser/' opys/BUG-0002.md
opys close BUG-0002                         # `close` needs an edge from where the
                                            # document stands into the terminal
                                            # status, and its `## Tasks` checked;
                                            # deletes the file, strikes the reference

# Claim a document atomically: one status transition, one field write and one
# log entry, applied together or not at all. Two agents racing for the same
# document cannot both win — `--status` asserts a *transition*, so the second
# finds it already taken and the whole batch is refused, writing nothing.
# (`claimed_by` is an ordinary declared field; the default config ships
# `[types.task.fields.claimed_by]` commented out, so uncomment it first.)
sed -i 's/^# \(\[types.task.fields.claimed_by\]\)/\1/; s/^# \(type = "string"\)/\1/' opys.toml
opys new --type task --title "Parse the OSC 2 terminator" --features FEAT-0001
opys apply TASK-0003 --status in-progress --field claimed_by=agent-7 --log "picked up"
sed -i 's/- \[ \] First item/- [x] the terminator is parsed/' opys/TASK-0003.md
opys set-status TASK-0003 review            # the claim's own log entry is the
                                            # checkpoint the handoff rule wants

# Bulk: every id-taking mutating command (set-status, tag, retire, block,
# unblock, close, apply) takes an id list — several ids, a comma-separated list,
# or `-` for stdin. All-or-nothing: one bad id writes nothing. (`log` is the
# exception: one id, because its second positional is the entry text and `-`
# there means stdin.)
# Filter on a status a live doc can hold — `done` is reached only *through*
# `close`, so it selects nothing, and an empty selection exits 2.
opys new --title "Report the icon separately" --tags osc
opys new --title "Blink the cursor on bell" --tags osc
opys set-status FEAT-0004 FEAT-0005 wontfix --reason "superseded"
opys list --type task --status review --format ids | opys close -
```

Mutating commands (`new`, `set-status`, `tag`, `log`, `apply`, `retire`,
`block`, `close`, `cleanup`, and `verify --fix` on a run that fixed
something) reconcile cross-references, linkify prose, and relocate
documents to their canonical layout path (e.g. an archived doc moves into
`_archived/`) automatically; pass `--no-sync` to skip, or run `opys sync` after
editing files by hand.

Ids come from one global, never-reused sequence. A project that creates
documents on several branches at once sets `[ids] modulus = M` in `opys.toml`:
each allocator then draws only the numbers of its slot (`OPYS_ID_SLOT`, else a
stable hash of `user@host`, modulo `M`), so branches cannot collide and merge
without a `renumber`. Without a modulus, allocation is the dense sequence.

## Commands

| Command | Purpose |
|---|---|
| `init` | bootstrap `opys.toml` + `opys/`, print a CLAUDE.md snippet |
| `config <init\|validate>` | generate / validate the universal `opys.toml` |
| `new --type <T>` | allocate the next ID and write a skeleton document of type `T` (auto-syncs) |
| `import --type <T>` | bulk-create documents of type `T` from a JSONL file (one ID block, one sync) |
| `show` / `list` | retrieval (`--type`, `--tag`, `--status`, `--field`, `--blocked`/`--unblocked`, `--sort`, `--format table\|ids\|paths`) |
| `next --status <s>` | the pick oracle: which documents at a status are pickable, best first — unblocked, with their type's `requires_link` still satisfied, ordered by the type's `order_by` chain. `--type`, `-n` (default 10), `--explain [ID]` for why a document is or is not listed, `--json` for the stable `opys.next/1` payload. A read: it exits `0` for every answer, an empty one included |
| `set-status` | guarded transitions: the type's declared `[transitions]` edges (when it declares any), its `max_visits` budget for the target status (the refusal names the configured `then`), and its configured rules |
| `tag` | add/remove tags (`--add a,b --remove c`) |
| `log` | append a dated entry to a document's `kind = "log"` section (`opys log TASK-0042 "…"`, or `-` to read it from stdin) |
| `apply` | several writes as one all-or-nothing batch — a status transition, `--field`/`--remove-field` writes and a `--log` entry, flushed once or not at all. How an agent claims a document: `--status` asserts a *transition*, so a second claimant is refused and writes nothing. Releasing is the same shape backwards, and stays one command even where the working status `require_field`s the claimant: the verify gate is taken once around the whole batch, so every part is judged against the document it finishes with |
| `retire` | delete document(s); each ID is logged and never reused |
| `block` / `unblock` | record a directional blocker between documents; blocked is derived from the live entries, never stored |
| `close` / `cleanup` | finish document(s) of a type with a terminal status (and, where a status graph is declared, an edge into one from where the document stands); strip struck refs |
| `verify [--fix]` | full integrity check — wire into CI. Exit `0` clean, `1` content problems, `2` a real failure, `3` [time observations](#time-aware-verify-stuck-and-overflowed-documents) only; `--fix` performs the fixable ones and exits on what remains |
| `sync` | reconcile references, linkify prose, relocate docs to their layout path (for hand edits) |
| `renumber [--base REF]` | reconcile ids two branches allocated twice: keep the ones already at the merge-base, renumber the branch-added duplicates, retire the old numbers, and warn (with a `sed` fix per file) about code still citing them |
| `history <id>` | the document's status timeline across commits, read out of git in-process (gix, never a spawned `git`), following the file across relocations |
| `history --seed <id>` | one-time migration: write the `history` lifecycle map from that timeline; refuses to overwrite an existing map (see [Upgrading an existing inventory](#upgrading-an-existing-inventory)) |
| `stats` | render configured `[[stats]]` sections (each a SQL query over the corpus, shown as a table; default: status counts, coverage, tags) |
| `query "SELECT …"` | run a read-only SQL query over the inventory (`-` reads it from stdin; `--stdin` binds stdin to `$1` for escape-free values) and print the result table. Every statement must be a SELECT — writes are lifecycle commands, so bulk edits pipe `list --format ids` into one. The `blocks` table decomposes bodies into `##` sections, for querying into prose |
| `agent-rules --tool <editor>` | generate a rules-based editor's instruction file from the canonical rule |
| `web <start\|add\|remove\|list\|scan\|install\|uninstall>` | the always-on node: serve the allowlisted projects over HTTP — see [below](#the-always-on-node-opys-web) |

Both `history` forms read git **in-process through gix**, never by spawning
`git`: the document's blob is found under the inventory base by *filename* —
which is its id — so the relocation a status change causes is not read as a
delete plus an add, and only the revisions that differ are decoded, through the
real parser. That same engine-side reader also answers which branches and
worktree heads hold the inventory and where they disagree with main, which is
what `[git]` in `opys.toml` configures (the truth branch, and which refs count:
local heads by default, remote-tracking refs opt-in). The dashboard's union view
surfaces exactly that: a column per branch nobody has checked out, beside the
columns for the worktrees, and a mark on each row saying which refs hold it at a
status main's copy does not. That reader degrades rather than failing: a project that
is not a repository, or a binary built without the `git` cargo feature
(`--no-default-features`), is main-only and says so — and a repository that *is*
there and cannot be read (a permission bit, a half-written `.git/config`) says
that instead, in different words, because it can clear on its own and the other
two cannot. The `history` *command* is
the exception — its whole subject is the committed timeline, so rather than
print an empty one it says why and exits 2, naming the missing feature or the
missing repository.

A feature file looks like (`history` and the `references` map are
auto-maintained — `history` records every status change, and a work item that
links back appears in `references`, struck through once it is closed):

```markdown
---
id: FEAT-0421
status: implemented
tags: [osc, tabs]
history:
  "2026-06-16T14:30:00.000000Z": planned
  "2026-07-01T09:00:00.000000Z": implemented
references:
  TASK-0042: Make tab title survive profile switch
---

# Tab title follows OSC 0/2 sequence

## Test plan
- [x] OSC 2 with valid UTF-8 updates title — `tab::osc_title_updates`
- [ ] Invalid UTF-8 in title payload — uncovered
```

See `skills/opys/references/format.md` for the normative document format and the
`opys.toml` config reference.

## The lifecycle, end to end

A type's `[transitions]` map *is* its lifecycle: which status may follow which.
Declare it once and four places read it — `set-status`, `apply`, `close`, and
`verify` over each document's recorded history — so they cannot disagree about
the route. This is what the shipped `task` type declares:

```toml
[types.task.transitions]
todo        = ["in-progress"]
in-progress = ["review", "todo"]
review      = ["done", "in-progress", "user-review"]
user-review = ["done", "in-progress"]

[types.task.fields.claimed_by]   # ships commented out — uncomment it to claim
type = "string"
```

**Create.** There is one way in: a document takes its type's `default_status`,
and `new` has no `--status` (`import` is the seam that seeds elsewhere).

```console
$ opys new --title "Tab title follows OSC 0/2" --tags osc,tabs
/home/dan/work/demo/opys/FEAT-0001.md
$ opys new --type task --title "Wire the OSC parser" --features FEAT-0001
/home/dan/work/demo/opys/TASK-0002.md
$ opys new --type task --title "Refactor the tab store" --features FEAT-0001
/home/dan/work/demo/opys/TASK-0003.md
```

**Claim it, in one command.** `opys apply` applies a transition, field writes
and a log entry as one all-or-nothing batch. Its `--status` asserts a
*transition* — the document must not already hold it — so of two agents racing
for the same document exactly one wins and the loser writes nothing at all:

```console
$ opys apply TASK-0002 --status in-progress --field claimed_by=agent-7 --log "picked up"
TASK-0002 -> in-progress, claimed_by set, logged to '## Progress'
$ opys apply TASK-0002 --status in-progress --field claimed_by=agent-9 --log "picked up"
error: cannot move TASK-0002 from in-progress to in-progress: it is already in-progress, and a composite write asserts a transition — another writer got there first
```

**Off the route, nothing moves.** Every refusal names where the document stands
and where it may go, so an agent takes the route from the message rather than
guessing at one. TASK-0003 is still where `new` left it, at `todo`, and `review`
is two moves away:

```console
$ opys set-status TASK-0003 review
error: cannot move TASK-0003 from todo to review: type 'task' declares no such transition (from todo: in-progress)
$ opys close TASK-0003
error: cannot close TASK-0003 from todo: type 'task' declares no transition from there to a terminal status (done) — only review, user-review can close
```

**Log a checkpoint before handing off.** `## Progress` is a `log` section: its
bullets are dated, `opys log` stamps them, and `verify` reports one written by
hand. The shipped config makes a *fresh* entry a precondition of the handoff
into `review` — one stamped since the status last changed — so whoever picks the
document up reads what happened instead of guessing:

```console
$ opys set-status TASK-0003 in-progress
TASK-0003 -> in-progress
$ opys set-status TASK-0003 review
error: cannot set TASK-0003 to review: '## Progress' has no log entry since the status last changed (append one with `opys log`)
$ opys log TASK-0003 "reproduced; the store keeps a stale title"
TASK-0003 logged to '## Progress'
$ opys set-status TASK-0003 review
TASK-0003 -> review
```

Or fold both into one composite write, which appends the entry *before* it
evaluates the rule that demands one — and releases the claim in the same breath:

```console
$ opys apply TASK-0002 --status review --remove-field claimed_by --log "parser wired; tests green"
TASK-0002 -> review, claimed_by removed, logged to '## Progress'
```

**Close.** A terminal status is reached only *through* `close`, which deletes
the ephemeral document and strikes its reference through in the feature it was
linked to. The type's own guards still apply:

```console
$ opys close TASK-0002
error: cannot close — unchecked items remain in '## Tasks' (check them, or pass --force)
$ opys close TASK-0002                      # with `## Tasks` checked
closed TASK-0002 (deleted; references struck through)
$ opys verify
verify: OK (2 documents)
```

Every one of those moves was appended to the document's `history` map — an
auto-maintained frontmatter map of UTC timestamp → status that nothing
hand-edits. It is what `verify` re-checks each consecutive pair of against the
graph, and what the time observations below are measured from.

## Roles and queues

Roles are plain names declared in `opys.toml`, with an optional description.
A type can map each non-terminal status to one role, several roles (any may
pick it), or `[]` (deliberately waits on nobody). Nothing changes until a
project opts in:

```toml
escalate_to = "human"

[roles.human]
description = "Sign-offs and decisions"
[roles.agent]

[types.task.owners]
todo = "agent"
review = "human"

[types.task.fields.owner]
type = "role"
```

This extends a task type whose statuses are `todo`, `review`, and terminal
`done`. With one task moved to review, the actual command output is:

```text
$ opys queue human
human: 1 waiting

task · review  (human)
  TASK-0001  Review the release spec  since 2026-10-19
$ opys list --waiting-on human --format ids
TASK-0001
```

Set `OPYS_ROLE=agent` in a harness and start with `opys queue`; `--json`
returns the stable `opys.queue/1` payload. Groups use the same pickable items
and ranking as `opys next --status review --type task --role human`, and show
blocked and orphaned documents separately. The escalation role also receives
attention for orphans, time observations and verification findings. `-n`
limits each group's items; attention and the distinct-document total remain
complete.

An optional `owner` role field overrides a document's status owners for one
step. Every move to a different status clears it; `opys apply TASK-0001
--status review --field owner=human` writes the override for the status it
enters. A same-status `set-status` keeps it. Write overrides only with
`apply` or the node's set-field action.

The node's **Role queues** page merges matching role names across projects,
shows attention first, and remembers the role chosen in this browser. Owners
also appear on flow nodes and beside the document's status. The node serves
identical per-project queue JSON at `/api/projects/{key}/queue/{role}` and
cross-project views at `/api/roles` and `/api/roles/{role}`.

## The pick oracle (`opys next`)

`opys next --status <s>` answers one deterministic question: **given a status,
which documents are pickable, best first.** Pickable means at that status, with
nothing live blocking it, and with its type's `requires_link` still satisfied.
The order is the type's `order_by` chain — the same ranking `list --sort` uses,
and the node's flow view lists each status node through this very function, so
the three cannot put one set in three orders.

The corpus below is a fresh one, not the one the walkthrough above just closed
documents in: its `task` type also declares `[types.task.fields.priority]`
(`type = "int"` — `order_by`'s priority key reads a declared field, and the
shipped config declares none), TASK-0002 carries `priority: 1` and blocks a
fourth task, and all three tasks are still at `todo`.

```console
$ opys next --status todo
| # | ID | TYPE | STATUS | PRIORITY | BLOCKS | TITLE | NOTES |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | TASK-0002 | task | todo | 1 | 1 | Wire the OSC parser |  |
| 2 | TASK-0003 | task | todo | - | 0 | Refactor the tab store |  |
```

`--explain` says why, one line per listed document — and with an id, why *that*
one is missing:

```console
$ opys next --status todo --explain
[the same table, then:]
TASK-0002: listed 1 of 2 — priority 1, blocks 1 live document
TASK-0003: listed 2 of 2 — no priority, blocks 0 live documents
$ opys next --status todo --explain TASK-0004
[the same table, then:]
TASK-0004: not listed — blocked by TASK-0002
```

It is a **read**: it never syncs, never writes, and exits `0` for every answer,
an empty one included — while a status or type no configured type declares is a
usage error rather than an empty list, because an oracle that answers "nothing"
to a typo cannot be told apart from an empty queue. `--json` is the stable
`opys.next/1` payload, carrying each item's priority, effective priority, blocks
count, position and time observations, plus every orphan with its reason.

Pickability is decided on **main**, the corpus the command runs in: a blocker
closed on a branch releases nothing until the merge lands. And `next` decides
nothing about *who* picks the work up — concurrency, affinity and budgets are
the caller's policy, which is why the answer is JSON when you ask for it.

## Time-aware verify: stuck and overflowed documents

A corpus is valid when it is written and *becomes* wrong with no write at all: a
document sits in `in-progress` for a week because whoever took it is gone, or
bounces between `review` and `in-progress` forever because the coder and the
reviewer are both agents. Those are not content problems, so a type can declare
what "too long" and "too often" mean for it and `verify` reports them apart:

```toml
[types.task.ttl]
in-progress = { after = "2d", then = "stuck" }   # or a bare "2d" to observe only

[types.task.max_visits]
review = { limit = 3, then = "user-review" }

[types.task.transitions]      # the moves those targets name have to be legal
todo        = ["in-progress"]
in-progress = ["review", "stuck", "user-review"]
stuck       = ["in-progress", "todo"]
review      = ["in-progress", "user-review", "done"]
user-review = ["review", "in-progress", "done"]
```

(`stuck` is not in the shipped `statuses` list — a target has to be a status the
type declares, so add it there too.)

```console
$ opys verify
verify: 2 observation(s)
  TASK-0001: stuck in in-progress since 2026-06-16T14:30:00.000001Z (ttl 2d) — `--fix` moves it to stuck
  TASK-0002: 3 visits to review reaches its limit of 3 — `--fix` moves it to user-review
$ echo $?
3
```

`verify --fix` performs those transitions itself — an ordinary `set-status`
each, history and relocation included — and exits on what remains:

```console
$ opys verify --fix
fixed TASK-0001: in-progress -> stuck (stuck)
fixed TASK-0002: review -> user-review (max_visits)
verify: OK (2 documents)
$ echo $?
0
```

Both moves are deterministic — an elapsed `ttl` and a spent `max_visits` budget
are computed from the document's own `history` and a clock — which is why opys
performs them itself and needs no orchestrator to be a complete loop tracker.
It is deliberately conservative: at most one fix per document per run, a refusal
(a `then` whose rules the document cannot satisfy) is reported as a problem
rather than aborting the pass, a run that fixed nothing writes nothing at all,
and it stands down entirely while `opys config validate` has anything to say —
a fix is a move along the graph that check guards.

Where the type also declares `[transitions]`, `config validate` insists the
graph holds the edges those moves need: `key -> then` for the fix itself,
`then -> key` so a document flipped under a running agent is one `set-status`
from where it was, and — for `max_visits` — an edge to `then` from every status
that may enter the budgeted one, because the guard refuses the *entry* and the
escape it names has to be legal from wherever the document stands.

**Exit codes:** `0` clean, `1` content problems, `2` a real failure (bad flags,
IO, a missing config), `3` observations and nothing else. Gate CI on **`1`**:
most scripts gate on any nonzero code, which trips on `3` as well, and a corpus
that only has observations was clean when it was written. `OPYS_NOW` (RFC3339)
overrides the clock, so a rerun of one commit always says the same thing.

## Upgrading an existing inventory

Three reserved things arrived after opys' first release, and an inventory
written before them needs a one-time pass for each. Only one of the three goes
red on the binary upgrade alone: the `history` lifecycle map is reserved on
every type, so no config declares it and `verify` starts saying `missing
history` the moment you install the new binary. The other two wait for your own
`opys.toml`. Bullets are dated and checked (`undated log entry`) only in a
section the type declares `kind = "log"`, and `blocked` only becomes an `invalid
status 'blocked'` once you drop it from a type's `statuses` — which is what
retiring it means, now that blocked is derived from the `blocked_by` relation.

None of the three is derivable from the corpus, so `opys sync` cannot do them
for you. Run them in this order, with `opys verify` clean and `opys sync` twice
leaving an empty diff at the end:

1. **Date the log bullets.** In each section the type declares `kind = "log"`,
   `- YYYY-MM-DD: text` becomes `- YYYY-MM-DDT00:00:00Z text`, and an undated
   `- text` takes the document's `created` (as UTC seconds) or the stamp of the
   bullet above it, whichever is later. Equal stamps are fine; continuation lines
   are left alone.
2. **Retire `blocked`.** Drop it from each type's `statuses` (and from
   `status_dirs`, `ttl`, `max_visits` and `transitions`), drop the
   `blocked_from`/`blocked_reason` fields and the rule that guarded the status,
   and give every permanent type a `releases_blocks` list. Then, per document at
   `status: blocked`: set `status` to its `blocked_from` value (or the type's
   `default_status`), delete `blocked_from` and `blocked_reason`, and rewrite any
   `blocked` value in its `history` map to the status it now holds. Leave
   `blocked_by`/`blocks` alone — they are what blocked is derived from now.
3. **Seed `history`.** `opys history --seed <id>` writes each document's map from
   its git timeline (`for id in $(opys list --format ids); do opys history --seed
   "$id"; done`) and refuses to overwrite one. For a document git cannot answer
   for, add a single entry by hand: the key is `created` in UTC with six
   fractional digits (`"2026-09-06T03:56:36.000000Z"`), the value is the current
   `status`, and the block sits in the frontmatter's canonical position (after
   `blocked_by`/`blocks`, before the remaining keys alphabetically). Seed last:
   step 2 rewrites `blocked` out of the maps this writes.

`opys verify` also gained exit code **3** for time observations. CI that gates
on "any nonzero code" starts failing on a corpus that has no content problems at
all; gate on `1` (see [above](#time-aware-verify-stuck-and-overflowed-documents)).

## The always-on node (`opys web`)

Every `opys` command so far is one shot: load the inventory, write, exit. The
**node** is that same engine kept warm — a long-lived local process that serves
the projects you allowlisted over HTTP, with a web dashboard, a typed API and a
live event stream. It is what you open when you want to see every project at
once instead of grepping one repo at a time. It ships inside the `opys` binary:
if you installed the CLI, you already have it.

### From nothing to a dashboard

**1. Look at the allowlist.** On a machine that has never run the node it is
empty, and an empty allowlist means the node would serve nothing at all:

```
$ opys web list
allowlist: /home/dan/.config/opys/server.toml
bind:      127.0.0.1:6797 (default)

nothing allowlisted — add a project with: opys web add <path>
```

**2. Allowlist a project** — any directory holding an `opys.toml`:

```
$ opys web add ~/work/notes
added /home/dan/work/notes to /home/dan/.config/opys/server.toml
a running node picks this up within a minute
```

All that did was write two lines to `~/.config/opys/server.toml`. Nothing was
started, and nothing was contacted:

```toml
[[project]]
path = "~/work/notes"
```

`opys web list` now prints the allowlist as written, and under it what those
entries resolve to right now:

```
$ opys web list
allowlist: /home/dan/.config/opys/server.toml
bind:      127.0.0.1:6797 (default)

  project  ~/work/notes  -> /home/dan/work/notes

serving 1 corpus in 1 project:
  notes  /home/dan/work/notes
```

**3. Start the node.** It runs in the foreground and `Ctrl-C` stops it; make it
a background service once you like it ([below](#run-it-as-a-service)):

```
$ opys web start
opys-server listening on http://127.0.0.1:6797
opys-server: serving 1 corpus from /home/dan/.config/opys/server.toml
```

**4. Open <http://127.0.0.1:6797>.** That is the dashboard.

### Why allowlisting is a separate step

This is the part that surprises people: `opys web start` takes no project paths,
and the node finds nothing by itself. It serves exactly the entries in
`~/.config/opys/server.toml` — a file only you write. Approving a project and
running the node are deliberately two different acts, because that file is the
security boundary. Two guarantees follow from it:

- **The node serves only what you allowlisted.** `opys web add` edits that file
  and never contacts a running node; the node re-reads the file on its own and
  picks up the change within a minute, no restart. So allowlisting is something
  you do at a terminal — never something a page open in your browser can do to
  you. Discovery only ever *suggests*: `opys web scan` prints candidates and has
  no way to add one.
- **The API is typed; the node cannot execute arbitrary commands.** Every write
  the node accepts is a named action with named arguments — `new`, `set-status`,
  `tag`, `log`, `apply`, `block`, `unblock`, `close`, and the three edit-in-place
  actions behind the document view (`edit-body`, `set-field`, `remove-field`) —
  run through the same engine, the same inventory lock and the same write-time
  rules as the CLI. (The dashboard sends a subset of them; the rest are there
  for an API client.) The request
  body is a closed set: there is no shell endpoint, no "run this opys command"
  endpoint, and no endpoint anywhere that accepts a filesystem path.

### More than one project

`opys web scan` walks your home directory (ten levels, skipping hidden, build,
vendor and cache directories), lists every project it finds and marks the ones
already allowlisted. It suggests and nothing more — the command cannot add
anything:

```
$ opys web scan
scanning /home/dan (depth 10)…
  /home/dan/Projects/opys
  /home/dan/Projects/opys-feature
  /home/dan/work/notes  (allowlisted)

scan never adds anything — allowlist one with:
  opys web add /home/dan/Projects/opys
```

Add them one `opys web add` at a time, or allowlist a whole tree with
`--prefix`, which covers everything ten levels below it — including projects you
create there later, found by the node's hourly rescan:

```
$ opys web add --prefix ~/Projects
added /home/dan/Projects to /home/dan/.config/opys/server.toml
a running node picks this up within a minute
```

One entry can serve several *corpora* — a corpus is one inventory: one
`opys.toml` and the documents under it. Sibling **git worktrees come along with
the project they belong to**, so allowlisting a repo covers every worktree of
it. Here two entries serve three corpora:

```
$ opys web list
allowlist: /home/dan/.config/opys/server.toml
bind:      127.0.0.1:6797 (default)

  project  ~/work/notes           -> /home/dan/work/notes
  prefix   ~/Projects (depth 10)  -> /home/dan/Projects

serving 3 corpora in 2 projects:
  opys   /home/dan/Projects/opys  main  (primary)
  opys   /home/dan/Projects/opys-feature  feature/web
  notes  /home/dan/work/notes
```

`opys web remove <path>` takes an entry back out. A project reached *through* a
prefix has no entry of its own, so instead of pretending, the CLI names the
entry that is responsible:

```
$ opys web remove ~/Projects/opys
not allowlisted directly — served by the prefix entry ~/Projects
remove that entry instead: opys web remove ~/Projects
```

Start the node again (or leave it running and wait a minute) and it serves all
three:

```
$ opys web start
opys-server listening on http://127.0.0.1:6797
opys-server: serving 3 corpora from /home/dan/.config/opys/server.toml
```

### What the dashboard shows

The sidebar lists every project and the corpora inside it — labelled by git
branch, with the primary worktree marked, and a dot per corpus for its verify
state (clean, *N* problems, or not read yet). Pick one and you get:

- **the board** — every document in that corpus, in a column per status, with
  filters for type and tag, a text filter set from the omnibox, and drag and
  drop: onto another column to change status, within a column to set priority
  (an opt-in field; see ADR-0046). The keyboard drives it too — arrows move
  between columns and cards, Enter opens, Home/End jump within a column,
  PageUp/PageDown switch projects;
- **a document** — its frontmatter and rendered body, both edited in place:
  status, tags, blockers and custom fields on the panel, the markdown body one
  block at a time — double-click a paragraph or list item to edit its
  markdown in place, tick checkboxes directly, click outside to save (there
  is no cancel) — and close behind a confirmation. A document whose type
  declares a `log` section gets an entry box under the body, which appends a
  timestamped entry the way `opys log` does. Every write is a typed
  action taking the same write path as the equivalent `opys` command, so a
  write the CLI would refuse — a status change whose rule is unmet, say — is
  refused here too, with the same message. Creating documents stays a CLI job.
- **the query console** — the same SQL over the corpus that `opys query` runs,
  read-only;
- **the union view** — every worktree of one project side by side, *plus a
  column for every branch nobody has checked out*, so you can see where two
  branches disagree about a document even when only one of them is on disk. The
  branch columns are read straight out of git objects by a background reader (one
  per project: a watcher on the git directory, a 250 ms debounce and a minute's
  backstop), so they cost the request nothing and refresh themselves when a ref
  moves. Each row also carries what is *in flight* on it — which refs hold it at
  a status main's copy does not, and since when. Both are measured against the
  worktree that has **main** checked out: a branch column shows that worktree's
  documents with the branch's own changes over the top, so an uncommitted edit
  there shows through on the rows the branch did not touch, and a project none of
  whose served worktrees is on main gets a notice instead of branch columns
  rather than a comparison against some third branch. A worktree column's heading
is the branch that checkout is on *now*, refreshed by the same reader on the
same event — so it cannot go on spelling `main` above a notice saying nothing is
on main. Three blanks mean three
  different things and are drawn differently: a column that does not have the
  document, a column that could not be read (with the reason on its heading), and
  — when there are no branch columns at all — one notice above the table saying
  why, because "this project is not a repository" is not "a branch broke". It
  shows the drift and nothing else: nothing here merges anything, because git is
  the merger, and a branch cell does not link anywhere because a branch is not a
  directory to open a file in.
- **the flow view** — the same project drawn as the lifecycle its config
  declares: a node per distinct status, merged by name across types (`task`,
  `bug` and `chore` share one `todo`), and an edge wherever some type's
  `transitions` map declares the move. A type that declares no transitions
  contributes its statuses and *no* edges — any status may follow any other,
  which is not the same thing as every edge. The layout is a small layered one:
  the statuses documents start at sit in the first column, each target one
  column past its deepest source, the cycles a left-to-right drawing has to
  break come back as dashed returns, and disconnected components — the shipped
  config draws two, for work items and features; this repository's own
  inventory draws two as well, for features and decisions — stack with a gap.
  Each node lists the top few documents standing on it, in `opys next` order and
  from the same function, so the view and `opys next --status <status>` cannot
  disagree; the rest are counted. An item's number is the one that ordered it —
  the effective priority, with the document's own beside it when it inherited a
  smaller one from something it blocks, exactly as the `PRIORITY` column of
  `opys next` reads. Blocked documents are counted and named on hover rather
  than listed, because they are not pickable, and stranded ones — the links
  their type requires no longer hold — are counted separately with the reason.
  A status no type declares gets a node too, marked `undeclared` (the blank one
  reads `(no status)`): `opys verify` reports it, and hiding the node here would
  hide the document with it. Clicking an item opens the document. It is answered
  from the project's **main** worktree, because that is where pickability is
  decided.

**Ctrl+P** (⌘P on a Mac) or `/` opens the omnibox from any view: a fuzzy
finder over the corpus's tickets — or every served corpus, from the home page
— that opens a ticket on Enter or, from a board, applies the text as its
filter. The project navigation toggle or `[` collapses the main sidebar into
a rail; narrow screens have a project-list toggle. The lifecycle's right panel
stays visible, showing details when a status is selected.

The dashboard is an installable web app: your browser's *Install* (or *Add to
Home Screen*) puts opys in its own window with its own icon. Installed, it still
opens while the node is down — onto a page that says so and reconnects when the
node returns; documents are never served from a cache. Browsers allow this only
in a secure context, which the default loopback address is; over plain HTTP on
another address the dashboard works as before but cannot be installed.

Everything updates live: the node watches each inventory and pushes events over
a WebSocket, so an edit you make in your editor — or a write from `opys` in
another terminal — shows up in the browser without a reload.

The port is **6797**, and the node binds loopback only. There is no
authentication, so the bind address *is* the boundary; while it is on loopback
the node also refuses any request whose `Host` is not loopback and any
cross-origin request, so a page you happen to be visiting cannot drive it.
Widen it — `opys web start --bind 0.0.0.0:6797`, or a `bind = "…"` line at the
top of the allowlist file — only if you mean to, and put something in front of
it that authenticates.

### Run it as a service

`opys web install` writes a systemd **user** unit and prints the two commands
that turn it on. It never runs them — enabling a service on your session is your
decision, not a side effect of an install:

```
$ opys web install
wrote /home/dan/.config/systemd/user/opys-server.service

enable it with:
  systemctl --user daemon-reload && systemctl --user enable --now opys-server

the node will listen on http://127.0.0.1:6797
```

Run those two commands and the node comes up at login and restarts if it
crashes. A **user** service lives and dies with your session, so on a machine
you are not usually logged into — a headless box you reach over SSH — also run
`loginctl enable-linger $USER`, or the node stops the moment you disconnect.

The unit is static — `ExecStart=…/opys web start --bind 127.0.0.1:6797`,
pointing at the binary you ran `install` from — so it never needs touching again
when you allowlist another project. Two things *are* fixed at install time: the
address (resolved then from `--bind`, else the allowlist file's `bind`, else the
default) and the `--config` path if you passed one. Change either afterwards and
re-run `opys web install --force`; editing `bind` in the allowlist file alone
will not move a service whose unit already names an address. Installing over an
existing unit is refused unless you pass `--force`:

```
$ opys web install
error: /home/dan/.config/systemd/user/opys-server.service already exists — pass --force to overwrite it
```

`opys web uninstall` deletes the unit and prints the disable line first, because
that is the order you have to run it in — deleting a unit file does not stop the
service it started:

```
$ opys web uninstall
stop it first — removing the unit does not stop a running service:
  systemctl --user disable --now opys-server && systemctl --user daemon-reload

removed /home/dan/.config/systemd/user/opys-server.service
```

On a machine with no systemd user manager — a Mac, a container, WSL1, a distro
that boots something else — `install` prints how to run the node by hand and
exits 0. That is a fact about the machine, not an error, and nothing is written:
a unit file no service manager will ever read is worse than no unit at all.

**On NixOS or with home-manager, do not run `opys web install`** — declare the
service instead, so it is reproducible and survives a rebuild rather than living
as an untracked file in `~/.config`. It is the same unit either way, so all you
are doing is writing it down where your configuration can see it.

First make `pkgs.opys` exist by applying this flake's overlay in your
configuration (`opys` here is this flake, taken as an input — see
[Use from another flake](#use-from-another-flake)):

```nix
nixpkgs.overlays = [ opys.overlays.default ];
```

Then, in **home-manager**, where the attributes are the unit's own sections:

```nix
systemd.user.services.opys-server = {
  Unit.Description = "opys always-on node";
  Service = {
    ExecStart = "${pkgs.opys}/bin/opys web start --bind 127.0.0.1:6797";
    Restart = "on-failure";
  };
  Install.WantedBy = [ "default.target" ];
};
```

or, in a plain **NixOS** configuration, where `systemd.user.services` is a typed
submodule rather than a freeform unit — same service, different spelling:

```nix
systemd.user.services.opys-server = {
  description = "opys always-on node";
  wantedBy = [ "default.target" ];
  serviceConfig = {
    ExecStart = "${pkgs.opys}/bin/opys web start --bind 127.0.0.1:6797";
    Restart = "on-failure";
  };
};
```

On a headless box add `users.users.<you>.linger = true;` (NixOS) for the same
reason `loginctl enable-linger` exists above.

If a served project sets `[ids] modulus`, the node's `new` action allocates in
the node's own slot: `OPYS_ID_SLOT` from the unit's environment when set, else
the hash of the account the service runs as (`$USER` at the hostname — from
`$HOSTNAME`, else `$COMPUTERNAME`, else `/etc/hostname`) — normally the same
slot as your own shell on that machine. On macOS, where none of those host
inputs is normally set, everything on the machine hashes as `user@localhost`,
so export `HOSTNAME` or pin a slot there. `opys web install` writes no slot
line, so to
pin one add `Environment=OPYS_ID_SLOT=<n>` under `[Service]` — as a drop-in
(`systemctl --user edit opys-server`), since `install --force` rewrites the
unit — or as `Service.Environment` / `serviceConfig.Environment` in the
declarations above.

The allowlist stays yours to edit either way: `opys web add` writes it, and the
node picks the change up without a restart. (One caveat if you hand-edit
`~/.config/opys/server.toml`: `opys web add`/`remove` rewrite the file from its
parsed form, which preserves keys and values but drops comments.)

### The `web` subcommands

| Command | Purpose |
|---|---|
| `web start [--bind ADDR] [--config PATH]` | run the node in the foreground |
| `web add <PATH> [--prefix]` | allowlist a project, or a directory to search under |
| `web remove <PATH>` | drop an entry from the allowlist |
| `web list` | the allowlist, and the corpora it currently resolves to |
| `web scan [--under PATH] [--depth N]` | suggest projects; adds nothing, ever |
| `web install [--bind ADDR] [--force]` | write the systemd user unit; print how to enable it |
| `web uninstall` | remove the unit; print how to disable it |

Every one of them except `uninstall` also takes `--config <PATH>`, to work on an
allowlist file other than `~/.config/opys/server.toml`. `install` writes that
path into the unit's `ExecStart`, so the service it installs serves the file you
named rather than the default one.

(`web scan` spells its scan root `--under` rather than `--root`, because `opys`
already has a global `--root` for the inventory root and clap propagates a
global into every subcommand. `--root` and `--no-sync` mean nothing to `web`:
no `web … --help` offers them, and writing one anyway — in any position, at any
value — is refused rather than ignored, because a scan of the wrong tree looks
exactly like a scan of the right one. The same surface is also available as
`opys-server web …` — one implementation, mounted by both binaries.)

## The `opys` skill

This repo doubles as a multi-agent plugin that drives `opys` (authoring
interviews, the implementation workflow, retrieval discipline). The skill lives,
once, in [`skills/opys/`](skills/opys/) and is
tool-agnostic; the repo also ships per-agent manifests so most tools can install
it natively. (The `opys` binary itself is a prerequisite — `cargo install opys`.)

**Native plugin/extension install:**

| Agent | Install |
|---|---|
| Claude Code | `/plugin marketplace add BohdanTkachenko/opys` then `/plugin install opys@opys` |
| Codex | `codex plugin marketplace add BohdanTkachenko/opys`, then install via `/plugins` |
| Gemini CLI | `gemini extensions install https://github.com/BohdanTkachenko/opys` |
| pi | `pi install git:github.com/BohdanTkachenko/opys` |
| opencode | add `"instructions": ["…/agent-rule.md"]` (see `opencode.json`) |

**Copy the skill folder** (conditional, fullest content) for tools that read a
skills directory:

| Tool | Copy `skills/opys/` to |
|---|---|
| Claude Code | `.claude/skills/opys/` (or `~/.claude/skills/`) |
| Cursor | `.cursor/skills/opys/` |
| Google Antigravity | `.agents/skills/opys/` |

```sh
git clone --depth 1 https://github.com/BohdanTkachenko/opys /tmp/opys
cp -r /tmp/opys/skills/opys <your-project>/.claude/skills/   # or .cursor/skills/ , .agents/skills/
```

**Always-on rule file** (a short, self-gating pointer — activates only when the
project has a `opys/` inventory) for rules-based editors: `opys` *generates*
it from one canonical rule (`skills/opys/agent-rule.md`), so there
are no duplicate files to keep in sync. Run it in your project:

```sh
opys agent-rules --tool cursor     # or windsurf | cline | copilot | kiro | all
opys agent-rules --tool copilot --stdout   # print instead of writing
```

It writes the right file in the right place (`.cursor/rules/opys.mdc`,
`.windsurf/rules/…`, `.clinerules/…`, `.github/instructions/…`,
`.kiro/steering/…`) with any host-specific frontmatter.

The skill folder carries the normative spec (`references/format.md`), so one
folder brings everything.

The CLI itself is universal — any agent that can run a shell command can use
`opys`. For tools that read project instructions instead of skills, the
cross-tool standard is **AGENTS.md** (this repo ships one). The substance is the
same everywhere: `opys new --type/set-status/close/verify ...` for writes,
`opys list`/`rg` for reads.

## License

Apache-2.0 — everything here, including the always-on node and its web UI.
Permanently, and for every crate in the workspace.
