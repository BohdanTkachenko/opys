---
retired:
  BUG-0023: close does not reserve an unreferenced doc's id
  BUG-0026: Corrupt _retired.md is silently read as an empty ledger
  BUG-0027: query --write can relocate files outside the inventory base
  BUG-0028: retire leaves inbound references dangling and the corpus failing verify
---

# Retired ids

Reserved ids that must never be reused. Managed by opys — the value is the document's last title; git records when and why each id was retired.
