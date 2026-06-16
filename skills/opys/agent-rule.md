# opys document inventory

This project may use **opys** — a file-based inventory of typed markdown
documents, configured by an `opys.toml` at the project root (which declares
where the documents live, default `docs/opys/`). Follow this when `opys.toml`
exists; otherwise ignore it.

- **Model.** One markdown file per document, with `---`-fenced YAML frontmatter
  (a stable `PREFIX-NNNN` id, status, tags, relation maps) and a markdown body.
  The document *types* — their id prefixes, statuses, fields, required sections,
  and validation rules — are declared in `opys.toml`. The default config ships a
  permanent `feature` type plus ephemeral `task`/`bug`/`chore` types that are
  deleted on `close`. Durable knowledge → features; "what I'm doing right now" →
  a task/bug/chore.
- **Reads.** Never bulk-read `docs/opys/`. Start at `docs/opys/INDEX.md`, then
  `rg` by tag/status, then open the 2–5 relevant files. `INDEX.md` and `views/`
  are generated — never edit them.
- **Writes go through the `opys` CLI** so invariants hold and parallel agents
  don't collide: `opys new --type <T>`, `set-status`, `tag`, `retire`, `block`,
  `close`. Body prose, `## Test plan`, and `## Tasks` edits are normal file
  edits. Run `opys verify` before finishing.
- **Never** put test results, dates, or completion claims in documents, or
  implementation logs in a permanent feature (those belong in a task/bug/chore).
- Full guide: the `opys` skill — `SKILL.md`, `references/format.md`. Install the
  CLI with `cargo install opys`.
