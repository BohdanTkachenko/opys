---
id: CHORE-0041
status: todo
created: "2026-07-06T05:14:48Z"
updated: "2026-07-06T05:14:48Z"
references:
  FEAT-0001: Config-driven document types
---

# Code hygiene: dead helpers, stale comments, init duplication

## Tasks
- [ ] remove dead `field_matches`/`scalar_str` in src/commands/mod.rs (no callers since list moved to SQL)
- [ ] fix the stale doc comment on src/rules.rs claiming "nothing in production calls it" (verify calls it on every run)
- [ ] deduplicate `init` vs `config init` (one should delegate to the other)
- [ ] wrap flush IO errors with the target path for diagnosability
- [ ] fix stale comment in pi-extension/index.js ("docs/opys/" — default base is opys/)

## Progress
- Filed from the pre-announcement review.
