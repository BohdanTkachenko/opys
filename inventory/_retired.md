---
retired:
  BUG-0023: close does not reserve an unreferenced doc's id
  BUG-0026: Corrupt _retired.md is silently read as an empty ledger
  BUG-0027: query --write can relocate files outside the inventory base
  BUG-0028: retire leaves inbound references dangling and the corpus failing verify
  TASK-0045: 'TUI: cut the built-in editor, spawn $EDITOR instead'
  TASK-0065: Remove the opys-tui crate
  TASK-0067: 'opys-server: scaffold the server crate'
  TASK-0068: 'engine: extract print-free cores for block/unblock'
  TASK-0069: 'opys-server: project discovery and the corpus registry'
  TASK-0070: 'opys-server: corpus actor with warm store and watcher'
  TASK-0071: 'opys-server: read API and WebSocket events'
  TASK-0072: 'opys-server: action endpoint through the command cores'
  TASK-0073: 'opys-server: worktree union view'
  TASK-0074: 'opys-server: embedded web UI'
  TASK-0075: 'opys-server: systemd install, docs, and M1 closeout'
  TASK-0110: 'engine: the history map and UTC sub-second timestamps'
  TASK-0111: 'engine: per-type transitions and the set-status guard'
  TASK-0112: 'engine: blocked is derived; remove the blocked status machinery'
  TASK-0113: 'engine: the ordering chain over the priority int'
  TASK-0114: 'engine: remove `query --write` and the writable blocks projection'
  TASK-0115: 'engine: `opys log` and dated log sections'
  TASK-0116: 'engine: ttl, max_visits, the stuck predicate, `verify --fix`, exit 3'
  TASK-0117: 'engine: the gix branch reader, default-on, and the comrak trim'
  TASK-0122: 'engine: interleaved id sequences'
  TASK-0123: 'engine: the atomic claim batch'
---

# Retired ids

Reserved ids that must never be reused. Managed by opys — the value is the document's last title; git records when and why each id was retired.
