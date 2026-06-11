# Skill: Feature Inventory (opys)

## Objective

Maintain this project's file-based feature inventory — one markdown file per
feature under `docs/features/`, managed by the
[`opys`](https://github.com/BohdanTkachenko/opys) CLI and verified in CI —
without corrupting generated artifacts.

## Rules of Engagement

- Source of truth is the feature files. `docs/features/INDEX.md` and
  `docs/views/` are generated — read them, never edit them.
- All metadata writes (new feature, status, tags) go through the `opys` CLI so
  invariants hold at write time and parallel agents don't collide. Never
  hand-edit frontmatter or status.
- Never record test results, dates, or completion claims in feature files.
- Run `opys verify` before considering work done; it is the CI gate.

## Instructions

1. To find a feature: read `docs/features/INDEX.md`, then `rg` by tag/status,
   then open only the relevant files. Do not bulk-read `docs/features/`.
2. To create one: `opys new --title "<title>" --tags <a,b>`.
3. To change status: `opys set-status <ID> <status> [--reason R]`
   (`implemented` requires ≥1 checked test-plan item; `wontfix` needs a reason).
4. To adjust tags: `opys tag <ID> --add <x> --remove <y>`.
5. When implementing a feature: read its file fully; implement; add tests;
   check the covered test-plan items and append backticked test references
   (`module::test_name`, a case may need several); then
   `opys set-status <ID> implemented` and `opys verify`.

Mutating commands regenerate `INDEX.md`/`views/` automatically.
