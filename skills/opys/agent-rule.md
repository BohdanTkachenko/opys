# opys feature inventory

This project may use **opys** — a file-based feature inventory, with optional
work items, under `docs/opys/`. Follow this when
`docs/opys/features/_config.toml` exists; otherwise ignore it.

- **Model.** One markdown file per feature (`FEAT-NNNN`) is the *permanent*
  record of what the project does. *Work items* (typed `TASK-`/`BUG-`/`CHORE-NNNN`,
  optional) are *ephemeral* per-change companions — tasks, a progress log, branch/PR links —
  deleted on completion. Durable knowledge → features; "what I'm doing now" →
  work items.
- **Reads.** Never bulk-read `docs/opys/`. Start at
  `docs/opys/features/INDEX.md`, then `rg` by tag/status, then open the 2–5
  relevant files. `INDEX.md` and `views/` are generated — never edit them.
- **Writes go through the `opys` CLI** so invariants hold and parallel agents
  don't collide: `opys new`, `set-status`, `tag`, `retire`; `opys work-item
  new`, `set-status`, `close`. Spec prose, `## Test plan`, and `## Tasks` edits
  are normal file edits. Run `opys verify` before finishing.
- **Never** put test results, dates, or completion claims in feature files, or
  implementation logs in a feature (those belong in a work item).
- Full guide: the `opys` skill — `SKILL.md`, `references/format.md`,
  `references/work-items.md`. Install the CLI with `cargo install opys`.
