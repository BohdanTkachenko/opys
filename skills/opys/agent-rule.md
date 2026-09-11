# opys document inventory

This project may use **opys** — a file-based inventory of typed markdown
documents, configured by an `opys.toml` at the project root (which declares
where the documents live, default `opys/`). Follow this when `opys.toml`
exists; otherwise ignore it.

- **Model.** One markdown file per document, with `---`-fenced YAML frontmatter
  (a stable `PREFIX-NNNN` id, status, tags, relation maps) and a markdown body.
  The document *types* — their id prefixes, statuses, fields, required sections,
  and validation rules — are declared in `opys.toml`. The default config ships a
  permanent `feature` type plus ephemeral `task`/`bug`/`chore` types that are
  deleted on `close`. Durable knowledge → features; "what I'm doing right now" →
  a task/bug/chore.
- **Reads.** Never bulk-read `opys/`. Use `rg` by tag/status (or `opys list`),
  then open the 2–5 relevant files. There is no generated index.
- **Writes go through the `opys` CLI** so invariants hold and parallel agents
  don't collide: `opys new --type <T>`, `set-status`, `tag`, `log`, `retire`,
  `block`, `close`. Body prose, `## Test plan`, and `## Tasks` edits are normal
  file edits. A **`log` section is not** — a `## Progress` is the usual one:
  its entries are dated, so append them with `opys log <id> "…"` (or `-` to read
  a multi-line entry from stdin), which stamps each one with the UTC time.
  `verify` reports an undated bullet and one whose stamp goes backwards, so a
  hand-written line turns the corpus red. Frontmatter is not a hand edit
  either: the relation maps and `history` (the lifecycle
  map recording every status change) are auto-maintained, so do not hand-edit
  them. opys reconciles the relation maps on every write — a hand-added link is
  kept, but its title is refreshed and the reverse side added, so editing them
  by hand buys nothing; a `history` map opys cannot read fails `verify` and is
  never repaired automatically — `set-status` refuses to run on that
  document until a human repairs the map. There is no blocked *status*:
  `block` records a relation, and a document is blocked while something
  live still blocks it (`opys list --blocked`), so everything that blocks
  is itself a document.
  A type may also declare a **status graph** (`[types.<name>.transitions]`):
  where it does, a document moves only along a declared edge, and `set-status`
  refuses anything else and names the statuses it will accept — take the answer
  from the refusal rather than guessing a route. `close` reads the same graph: it
  needs a declared edge from where the document stands into a terminal status, so
  a work item is moved along (to `review`, under the shipped config) before it is
  closed. A type may also cap how often a document may enter a status
  (`max_visits`); that refusal names the status the config escalates to instead,
  and it is always a legal move from where the document stands.
  Run `opys verify` before finishing. Exit 1 is content problems and must be
  fixed; exit **3** means the corpus is clean and only *time* observations
  remain (a document past its type's `ttl`, or at its `max_visits` limit) — run
  `opys verify --fix` to perform the transitions the config declares for them,
  or tell the user what is stuck.
- **Always say what an id is.** `PREFIX-NNNN` is a lookup key, not a name:
  a reader who sees `ADR-0082` learns nothing at all. Every time you write an id
  in prose — in a document body, a commit message, or a reply to a person —
  give its title with it: `ADR-0082 — The web UI may manage the allowlist within
  $HOME`. Look it up with `opys show <id>` if you do not have it; never guess a
  title, and never leave a bare id standing in for one. Bare is correct only
  where the id *is* the payload and a title would be noise: inside a command
  (`opys close TASK-0042`), a filename, a frontmatter key, or a table that
  already has a title column.
- **IDs in code.** When a `PREFIX-NNNN` id is cited in code (comments, tests,
  strings), `opys show <id> --refs` lists those mentions. If two branches
  allocated the same number, `opys renumber` reconciles them — it rewrites
  documents and warns (with `sed` suggestions) about any code references to fix.
  A project with `[ids] modulus` set avoids the collision up front: each
  allocator draws only its slot's numbers (`OPYS_ID_SLOT`, else `user@host`).
- **Never** put test results, dates, or completion claims in documents, or
  implementation logs in a permanent feature (those belong in a task/bug/chore).
- Full guide: the `opys` skill — `SKILL.md`, `references/format.md`. Install the
  CLI with `cargo install opys`.
