---
id: CHORE-0038
status: todo
created: "2026-07-06T05:14:48Z"
updated: "2026-07-06T05:14:48Z"
references:
  FEAT-0022: Multi-agent skill and plugin packaging
---

# Write the 0.12.0 changelog and deprecate the orphaned opys-core crate

## Tasks
- [ ] add a 0.12.0 section to CHANGELOG.md (the rename release opys-core → opys-engine currently has an empty Unreleased)
- [ ] publish a deprecation pointer for opys-core 0.11.x on crates.io (README stub or yank note pointing at opys-engine)
- [ ] publish a deprecation pointer for opys-tui on crates.io (retired per [ADR-0050 — Retire the TUI; the web UI over the server is the human surface](ADR-0050.md) / [TASK-0065 — Remove the opys-tui crate](TASK-0065.md); the crate left the workspace)
- [ ] route future version bumps through release-plz so changelog_update fires

## Progress
- Filed from the pre-announcement review: the 0.12.0 bump was made manually outside release-plz's release-pr flow, so the pending release would tag v0.12.0 with no notes.
