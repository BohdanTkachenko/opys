---
retired:
  BUG-0023: close does not reserve an unreferenced doc's id
  BUG-0026: Corrupt _retired.md is silently read as an empty ledger
  BUG-0027: query --write can relocate files outside the inventory base
  BUG-0028: retire leaves inbound references dangling and the corpus failing verify
  TASK-0045: 'TUI: cut the built-in editor, spawn $EDITOR instead'
  TASK-0065: Remove the opys-tui crate
  TASK-0067: 'opys-server: scaffold the AGPL crate'
  TASK-0068: 'engine: extract print-free cores for block/unblock'
  TASK-0069: 'opys-server: project discovery and the corpus registry'
---

# Retired ids

Reserved ids that must never be reused. Managed by opys — the value is the document's last title; git records when and why each id was retired.
