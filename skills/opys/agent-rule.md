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
  don't collide: `opys new --type <T>`, `set-status`, `tag`, `log`, `apply`,
  `retire`, `block`, `close`. Body prose, `## Test plan` and `## Tasks` edits are
  normal file edits. A **`log` section is not** — `## Progress` is the usual one:
  its entries are dated, so append them with `opys log <id> "…"` (or `-` to read
  a multi-line entry from stdin), which stamps each with the UTC time. `verify`
  reports an undated bullet and one whose stamp goes backwards, so a hand-written
  line turns the corpus red. Frontmatter is not a hand edit either: the relation
  maps are reconciled on every write, and `history` (the lifecycle map recording
  every status change) is appended to by the lifecycle commands, so editing
  either by hand buys nothing — and a `history` map opys cannot read fails
  `verify`, is never repaired automatically, and stops `set-status` on that
  document until a human fixes it. There is no blocked *status*: `block` records
  a relation, and a document is blocked while something live still blocks it
  (`opys list --blocked`), so everything that blocks is itself a document.
- **Move along declared edges, and log a checkpoint before handing off.** Where
  a type declares a status graph (`[types.<name>.transitions]`) a document moves
  only along a declared edge; the refusal names the statuses it will accept, so
  take the route from it rather than guessing. `close` reads the same graph — it
  needs a declared edge from where the document stands into a terminal status,
  so a work item is moved along (to `review`, under the shipped config) before
  it is closed — and a type may cap how often a document may enter a status
  (`max_visits`), that refusal naming the escalation the config declares. A type
  may also require a **fresh log entry before a transition**
  (`require_log_since_status`): under the shipped config the move into `review`
  is refused until `## Progress` holds an entry stamped since the status last
  changed. Log the checkpoint first, or fold it into the same command —
  `opys apply <id> --status review --log "…"` appends the entry before it
  evaluates the rule that demands one.
- **A claim is one command.** Taking a document — moving it into the working
  status, recording who took it in the field the project declares for that, and
  logging that you took it — is a single `opys apply <id> --status … --field
  … --log "…"`, applied as one all-or-nothing batch. Never spell it as three
  commands: `apply --status` asserts a *transition*, so if another agent claimed
  it first the whole invocation is refused (exit 2) and writes nothing, which is
  what stops two agents working the same item. Releasing and handing on are the
  same shape.
- **Run `opys verify` before finishing.** Exit 1 is content problems and must be
  fixed; exit **3** means the corpus is clean and only *time* observations
  remain (a document past its type's `ttl`, or at its `max_visits` limit) — run
  `opys verify --fix` to perform the transitions the config declares for them,
  or tell the user what is stuck.
- **Always say what an id is.** `PREFIX-NNNN` is a lookup key, not a name: a
  reader who sees `ADR-0035` learns nothing. Every time you write an id in prose
  — a document body, a commit message, a reply to a person — give its title with
  it: `ADR-0035 — The web UI may manage the allowlist within $HOME`. Look it up
  with `opys show <id>`; never guess a title. Bare is correct only where the id
  *is* the payload: inside a command (`opys close TASK-0042`), a filename, a
  frontmatter key, or a table that already has a title column.
- **IDs in code.** `opys show <id> --refs` lists code mentions of an id. If two
  branches allocated the same number, `opys renumber` reconciles them (with
  `sed` suggestions for code references); a project with `[ids] modulus` set
  avoids the collision up front, each allocator drawing only its slot's numbers
  (`OPYS_ID_SLOT`, else `user@host`).
- **Never** put test results, dates, or completion claims in documents, or
  implementation logs in a permanent feature (those belong in a task/bug/chore).
- Full guide: the `opys` skill — `SKILL.md`, `references/format.md`. Install the
  CLI with `cargo install opys`.
