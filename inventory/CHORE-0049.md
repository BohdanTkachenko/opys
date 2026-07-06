---
id: CHORE-0049
status: todo
created: "2026-07-06T16:11:20Z"
updated: "2026-07-06T16:11:20Z"
references:
  FEAT-0008: Structured sections (mdprism schemas)
---

# Trim mdprism to its used half

Decision: the structured-section feature stays (validate + scaffold are on-story and in production use); the speculative half goes. `edit`/`render` have zero production callers, carry the module's worst bugs, and their documented API does not match the code; the %strict/%frontmatter directives are parsed but never enforced.

## Tasks
- [ ] remove (or feature-gate out) mdprism edit.rs and render() plus their doc surface
- [ ] remove or implement the %strict / %frontmatter directives — do not ship parsed-but-unenforced switches
- [ ] fold a one-page structured-sections guide into format.md; demote docs/structure-dsl-spec.md and docs/mdprism-reference.md to internal design notes (or delete)
- [ ] revisit extracting mdprism as its own crate only if the feature earns adoption

## Progress
- Decided during the pre-announcement review discussion.
