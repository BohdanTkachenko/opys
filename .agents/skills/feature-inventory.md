# Skill: Feature Inventory (opys)

This project tracks features with **opys** — a file-based inventory under
`docs/features/`, managed by the `opys` CLI and verified in CI.

## Objective

Maintain the inventory without corrupting generated artifacts.

## Rules of Engagement

- Operate it **only** through the CLI — run `opys --help`; the writes are
  `new`, `set-status`, `tag`, `retire`. Never hand-edit frontmatter or status.
- `docs/features/INDEX.md` and `docs/views/` are generated — read them, never
  edit them.
- Run `opys verify` before finishing (it is the CI gate). Never record test
  results, dates, or completion claims in feature files.

## Instructions

- Find a feature: read `docs/features/INDEX.md`, then `rg`; open only the
  relevant files.
- Create / update: `opys new ...`, `opys set-status <ID> <status>`,
  `opys tag <ID> --add/--remove ...`.
- When implementing: add tests, append backticked test refs
  (`module::test_name`) to the covered test-plan items, then
  `opys set-status <ID> implemented` and `opys verify`.

Canonical workflow + normative format spec live in the opys skill —
`.claude/skills/feature-inventory/` — and at
<https://github.com/BohdanTkachenko/opys>.
