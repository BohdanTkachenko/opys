---
id: CHORE-0040
status: todo
created: "2026-07-06T05:14:48Z"
updated: "2026-07-06T05:14:48Z"
references:
  FEAT-0022: Multi-agent skill and plugin packaging
---

# Docs sweep before the announcement

## Tasks
- [ ] fix the SKILL.md command table — a paragraph is spliced mid-table, orphaning the verify/sync/renumber/stats/query rows
- [ ] make the skill folder self-contained: format.md links ../../../docs/{structure-dsl-spec,mdprism-reference}.md, dead once skills/opys/ is copied out
- [ ] update format.md's stats table list (four tables documented, seven exist) and the stale jq comment in src/templates.rs
- [ ] fix README line 19 "(create, verify, index)" — there is no index; add tags/renumber/history to the command table
- [ ] fix bulk-id help examples (FEAT-1 does not resolve; ids are padded) or normalize unpadded ids
- [ ] unify the framing: crates.io says "issue tracker", SKILL.md says "file-based JIRA", README says "not a task board" — pick the README's story everywhere
- [ ] document the 0/1/2 exit-code contract and a copy-paste CI snippet in the README
- [ ] state the missing-pieces story: no opys edit by design (hand edits + sync), comparison to alternatives, eject/durability note
- [ ] pre-announcement sanitization pass: erase git history to a fresh root commit, and re-read the tree for anything that describes plans rather than the tool
- [ ] purge remaining TUI references from all docs after [TASK-0065 — Remove the opys-tui crate](TASK-0065.md) lands

## Progress
- Filed from the pre-announcement review.
- Extended with the history-erase + sanitization pass and the
  TUI purge; the sweep targets the launch announcement, not the
  original CLI-only launch.
