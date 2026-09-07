# Seeding `history` on an existing inventory

`history` is a reserved, auto-maintained frontmatter key: a map of RFC3339 UTC
timestamp (six fractional digits, `Z`) to status, recording every status a
document has held (see the format spec, *The lifecycle map*). `new`,
`set-status`, `import` and `block`/`unblock`'s auto-set write it; **nothing
rebuilds it**. An inventory created before the key existed therefore has no
history at all, and `opys verify` reports one problem per document:

```
FEAT-0001: missing history (the lifecycle map is written by new/set-status/import; …)
```

This is a **one-time procedure an agent runs**, not a tool. `opys sync` cannot
help: unlike the relation maps, history is not derivable from the corpus, so
sync would have to invent it.

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

**Not yet implemented** — [TASK-0117](../inventory/TASK-0117.md) adds
`opys history --seed`, which reads each document's real status timeline out of
git (the same branch reader behind `opys history <id>`) and writes one entry per
observed transition, with the commit's author date as the key. Where git has the
timeline, that is the better history: it is the truth, not an approximation.

Until it lands, use the fallback below. Documents seeded from `created` can be
re-seeded from git later without loss — a full timeline simply replaces the
single entry.

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
remaining keys alphabetically, so `history` goes after `blocked_by` /
`blocked_from` / `blocks` and before `priority` / `references` / `spec` /
`wontfix_reason`. Position matters more than it looks: `opys` compares a file
against the *canonical rendering of what it parsed*, so a history block appended
at the end of the frontmatter is invisible to `verify` and `sync` — and then
silently reordered by the next unrelated write.

Get the shape right the first time: a `history` that is present and carries
entries but is not a lifecycle map — a key that is not canonical, a status that
is not a string, or a **list** of entries rather than a mapping — is **not**
repaired by any command. `verify` names the offending key or entry (or, for a
value that is not a mapping at all, its shape), and
`set-status` (and `block`/`unblock`'s auto-set) refuse to run on that document
until a human fixes it, rather than appending beside a value opys cannot read
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
