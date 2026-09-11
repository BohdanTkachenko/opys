> Two one-time upgrade procedures for an inventory that predates a reserved,
> auto-maintained field. Part 1 seeds the `history` lifecycle map; part 2 dates
> the bullets of every `kind = "log"` section. Both are procedures an agent
> runs by hand — neither is derivable from the corpus, so `opys sync` cannot do
> either.

# Seeding `history` on an existing inventory

`history` is a reserved, auto-maintained frontmatter key: a map of RFC3339 UTC
timestamp (six fractional digits, `Z`) to status, recording every status a
document has held (see the format spec, *The lifecycle map*). `new`,
`set-status`, `apply --status` (the same append inside a composite batch) and
`import` write it; **no ordinary write rebuilds it**. An inventory created
before the key existed therefore has no history at all, and `opys verify`
reports one problem per document:

```
FEAT-0001: missing history (the lifecycle map is written by new/set-status/apply/import; …)
```

This is a **one-time procedure**, and only half of it is a tool: `opys history
--seed` (§2) reconstructs the map where git has the timeline, and an agent fills
in the rest by hand (§3). `opys sync` cannot help either way: unlike the relation
maps, history is not derivable from the corpus, so sync would have to invent it.

## 1. Find the documents

```sh
opys verify 2>&1 | grep 'missing history'
```

The phrase `missing history` is stable — that is what it is for. Anything else
`verify` reports is a separate problem; fix those first, so the migration diff
is only the history blocks.

Take a baseline before you start: `opys verify` (expect only `missing history`
lines) and `opys sync` twice — `sync` prints `synced N document(s)` either way,
so the check is that your VCS diff is empty afterwards. Otherwise unrelated
reconcile/linkify churn is indistinguishable from the migration.

## 2. Preferred: seed from git

`opys history --seed <id>` reads a document's real status timeline out of git —
the same reader behind `opys history <id>`, in-process through gix — and writes
one entry per status the document held, keyed by the commit that introduced it
(the commit time, in UTC with six digits). Where git has the timeline, that is
the better history: it is the truth, not an approximation.

One document at a time, so a document git cannot answer for stops nothing:

```sh
for id in $(opys list --format ids); do opys history --seed "$id"; done
```

Each iteration is its own process, so the loop runs to the end; the ones that
refuse print why and exit 2, and you deal with them under §3. What it writes:

- **the first entry** is keyed from the document's `created`, not from the first
  commit — `verify` requires the two to denote the same second, and a document is
  essentially never committed in the second it was created. Its *status* is the
  one the earliest commit carried. A document with no `created` takes the first
  commit's instant instead, and the seed's own auto-sync pass writes `created`
  from that first entry in the same command — `--no-sync` is what defers it to
  the next write;
- **later entries**, one per status change, at the commit that introduced it. A
  key that is not strictly greater than the previous one is bumped by a
  microsecond, so commits that predate `created`, or two transitions in one
  second, still produce a strictly increasing map;
- **a final entry at the wall clock** when the working copy's `status` is ahead
  of the last commit — the ordinary mid-migration state, since the change that
  prompted the migration is usually uncommitted. `verify` requires the last entry
  to match `status`.

`updated` is not touched: a seed reconstructs the past, it is not an edit.

It **refuses**, writing nothing and exiting 2, in three cases:

- the document already carries entries (`--seed refuses to overwrite it`).
  History is not rebuildable from ground truth, so an existing map is data — the
  same promise every other write path makes. Re-seeding means deleting the block
  by hand first, deliberately;
- the `history` value is present but is not a lifecycle map — repair it by hand
  (§3) and retry;
- git has no committed revision carrying a status. That is the caveat for
  **documents created before the tool tracked status**: git can only show what
  was committed, and a revision whose frontmatter has no `status` (or does not
  parse) is skipped rather than guessed at. A document whose every revision is
  like that, or that has never been committed, has no timeline to seed from —
  use §3 for those.

Documents seeded from `created` under §3 can be re-seeded from git later, but
only after their block is removed by hand: `--seed` will not overwrite one.

## 3. Fallback: one entry from `created`

For each document `verify` named, add exactly one entry:

- **key** — the document's `created` value converted to UTC and written with six
  fractional digits and a `Z` suffix. `2026-09-05T23:56:36-04:00` becomes
  `"2026-09-06T03:56:36.000000Z"`. (`created` and the first key must denote the
  same second; verify checks it.) The six digits are required, not cosmetic:
  `verify` rejects any other width, because only fixed-width keys sort the same
  way lexically and chronologically and the serializer emits the block sorted.
  A document with no `created` may use any UTC key in that same canonical form —
  the next write (or `opys sync`) backfills `created` from it.
- **value** — the document's current `status`. (The last entry must equal
  `status`; with one entry, first and last are the same.)

The block is spliced into the frontmatter **in its canonical position**: the
serializer emits `id`, `status`, `tags`, `created`, `updated` first and then the
remaining keys alphabetically, so `history` goes after `blocked_by` / `blocks`
and before `priority` / `references` / `spec` / `wontfix_reason`. Position matters more than it looks: `opys` compares a file
against the *canonical rendering of what it parsed*, so a history block appended
at the end of the frontmatter is invisible to `verify` and `sync` — and then
silently reordered by the next unrelated write.

Get the shape right the first time: a `history` that is present and carries
entries but is not a lifecycle map — a key that is not canonical, a status that
is not a string, or a **list** of entries rather than a mapping — is **not**
repaired by any command. `verify` names the offending key or entry (or, for a
value that is not a mapping at all, its shape), and
`set-status` refuses to run on that document until a human fixes it, rather than appending beside a value opys cannot read
and destroying the rest of the map. A botched seed therefore wedges exactly the
documents it touched; it never silently loses them.

Two-space indent, key quoted, status bare:

```yaml
blocks:
  TASK-0111: 'engine: per-type transitions and the set-status guard'
history:
  "2026-09-06T03:56:36.000000Z": todo
references:
  FEAT-0104: 'Configured lifecycle: transitions, history, derived blocked, priority'
```

The quotes are load-bearing, and `verify` checks them — against the file, not
the parsed map, because opys reads YAML 1.2 and a bare
`2026-09-06T03:56:36.000000Z` resolves to the same string there:

```
FEAT-0001: history key '2026-09-06T03:56:36.000000Z' is written unquoted …
```

A YAML 1.1 reader resolves that key as a *timestamp* instead, so a corpus seeded
without the quotes is no longer the string→string map the format specifies for
every other tool that reads it — and nothing repairs it: the next unrelated
write re-quotes the block, turning the mistake into a surprise diff weeks later.

A short script is the right tool — one pass over the files `verify` named,
reading `created` and `status` out of the frontmatter and inserting the block at
the first key that sorts after `history`. Keep the script outside the repository:
it is run once and is not part of the inventory.

## 4. Check

```sh
opys verify          # OK (N documents)
opys sync            # prints "synced N document(s)" either way — the check is
                     # that your VCS diff is empty afterwards; run it twice
```

`verify` and `sync` are necessary but **not sufficient**: `verify` reads the
file for the one spelling that changes what the document *means* to another
reader (an unquoted key, §3), but neither command can see a **misplaced** block.
As §3 says, `opys` compares a file against the canonical
rendering of what it *parsed*, so a `history:` block written in the wrong slot
parses fine, renders canonically, and is rewritten only later, by an unrelated
command. To catch that you have to force a rewrite yourself — a no-op edit round
trip over every migrated document:

```sh
for id in $(opys list --format ids); do
  opys tag "$id" --add migration-probe
  opys tag "$id" --remove migration-probe
done
git diff --stat          # or your VCS's equivalent
```

Every document is rewritten canonically by that pass, so the only line that may
differ from the seeded file is `updated`. A `history:` block that *moves* was
written in the wrong slot: re-seed those documents and run the probe again.

The migration is not urgent, and it cannot be raced. A status change on a
document that still has no history seeds that document's first entry from
`created` automatically, at the status it is leaving, and appends the transition
second — so a document touched mid-migration comes out correctly seeded rather
than stamped with the wall clock. Every document still waiting keeps reporting
`missing history`, and never anything worse.

# Dating an existing inventory's log sections

A section declared `kind = "log"` holds a **dated timeline**: every top-level
bullet starts with an RFC3339 UTC timestamp at whole seconds (see the format
spec, *Log sections*). `opys log` stamps every entry it writes; nothing
rewrites an entry written before the command existed. So an inventory created
earlier reports one problem per old bullet, and keeps reporting it until this
procedure has been run:

```
TASK-0046: section 'Progress': undated log entry 'Decided during the pre-announce…' (every entry starts with a UTC timestamp — …)
```

Undated bullets are **reported, not rejected**: nothing refuses to write, and
the documents stay usable. But `opys verify` exits 1, so an inventory wired into
CI goes red on the upgrade — run this before or with it.

## 1. Find the documents

```sh
opys verify 2>&1 | grep 'undated log entry'
```

Only sections a type *declares* as `kind = "log"` are checked, so a stray
`## Progress` heading on a type that declares no log section is out of scope —
leave it alone. `opys query "SELECT doc_id, heading FROM sections WHERE kind =
'log'"` lists exactly the sections in scope.

Take a baseline first: `opys verify` (expect only `undated log entry` lines) and
`opys sync` twice, checking your VCS diff is empty afterwards, so unrelated
reconcile/linkify churn is not mistaken for the migration.

## 2. Rewrite the bullets

Per bullet, inside a declared `log` section only:

| the old bullet | becomes |
| --- | --- |
| `- YYYY-MM-DD: text` (the common hand-written form) | `- YYYY-MM-DDT00:00:00Z text` |
| `- text` (undated) | the document's `created` as UTC seconds — or the stamp of the bullet above it, whichever is **later** — then the text: `- 2026-07-06T05:14:48Z text` |
| already RFC3339 UTC | left alone |

Continuation lines — anything not starting with `- ` at column 0 — are never
touched: they belong to the bullet above them and carry no stamp of their own.

Three rules make this safe to do mechanically:

- **Equal stamps are legal.** The check is non-decreasing, not strictly
  increasing, precisely so that a document whose two undated bullets both take
  its `created` comes out green.
- **A stamp earlier than `created` is legal too**, as long as no bullet above it
  carries a later one. Nothing compares a bullet to `created`, so a
  `- 2026-08-24:` bullet on a document created on the 25th (a note written
  before the file existed) needs no adjustment.
- **An undated bullet never goes backwards.** `created` is usually *earlier*
  than the hand-written dates above it, so an undated bullet that follows a
  dated one cannot simply take `created` — a `## Progress` of `- 2026-08-24: did
  X` then `- finished Y` on a document created in January would come out
  `2026-08-24T00:00:00Z`, `2026-01-02T03:04:06Z` and fail verify with `log
  entries must not go backwards`. Carry the running maximum instead: each
  undated bullet takes `created` or the stamp of the bullet above it, whichever
  is later. In a log that is entirely undated the two are the same thing, which
  is the first rule again.

Order the bullets as they already stand in the file: the dates are an
approximation of when the work happened, and reordering them would lose the
sequence, which is the part that was real.

## 3. Check

```sh
opys verify        # the 'undated log entry' problems are gone
opys sync          # then again — the diff must be empty the second time
```

From here on, append with the command — `opys log <id> "…"`, or `opys log <id> -`
for a multi-line entry — and the stamps take care of themselves.

# Retiring the `blocked` status

A second one-time procedure, for an inventory written before blocked became
derived ([ADR-0099](../inventory/ADR-0099.md) decision 3). It is unrelated to the
history seeding above except in shape: a text pass an agent runs once, not a
tool.

Blocked used to be a *status*. `opys block` auto-set it, remembered the status it
came from in `blocked_from`, and a free-text `blocked_reason` counted as blocked
too. All three are gone: `block` writes the two relation maps and nothing else,
and a document is blocked while it holds a live `blocked_by` entry. So when
`blocked` leaves a type's `statuses` in `opys.toml`, every document sitting at it
turns red at once:

```
TASK-0112: invalid status 'blocked'
TASK-0112: unknown frontmatter field 'blocked_from' (declare it in type 'task' [fields.blocked_from])
```

The config change and the document pass have to land together. Per document at
`status: blocked`:

1. `status: blocked` becomes the value of its `blocked_from` line — the status it
   was auto-set away from — or the type's `default_status` when it has none.
2. the `blocked_from:` line is deleted. So is any `blocked_reason:`, unless the
   project keeps declaring it as an ordinary field.
3. in the `history:` map, an entry whose value is `blocked` is rewritten to the
   status the document now holds. These documents never really left the one they
   came from — the entry recorded the auto-set, not a decision — and leaving it
   is not free. The unconditional reason is the last-entry check: once the
   document moves off `blocked`, a trailing `blocked` entry no longer names the
   status it holds, and `verify` says so whatever the type declares —

   ```
   TASK-0112: history's last entry (blocked) does not match status (todo)
   ```

   That is the shape every document the auto-set touched has, since `block` wrote
   the entry last. A `blocked` entry with a *successor* fails a second check, but
   only where the type also gains a `transitions` map: the pair around it is no
   longer a declared edge (`history records blocked -> todo, which type 'task'
   does not declare as a transition (no transitions out of blocked)`).

`blocked_by` and `blocks` stay exactly as they are: they are what makes derived
blockedness work, and rewriting them would lose the dependency.

Nothing else moves — in particular not `created` and not the history *keys*,
which `verify` checks against each other. Then the usual close-out: `opys verify`
clean, and `opys sync` twice with an empty VCS diff.

Two consequences worth knowing before the pass, not after:

- If the same change gives the type a `transitions` map (as the shipped config
  now does), `close` works only from a status with a declared edge into a
  terminal one. Under the shipped task graph that is `review` and `user-review`,
  so `opys close` on a `todo` document is refused until it is moved along.
- A document blocked *only* because someone wrote a `blocked_reason` has no
  blocker to derive from. That is the point of the change: make the thing it is
  waiting on a document, and `opys block` it.
