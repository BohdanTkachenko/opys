# mdprism — reference (every feature)

A single worked example exercising every construct, then the same schema used to
validate, extract, query, edit, and scaffold. Companion to
[`structure-dsl-spec.md`](./structure-dsl-spec.md).

---

## 1. The kitchen-sink schema

Every feature appears at least once; the trailing `--` text on each line is its
description.

```
%ordered = true            # body nodes must appear in declared order (default)
%strict  = true            # error on mismatch / unexpected blocks (default)
%frontmatter = closed      # unknown frontmatter keys are errors (default)

--- # ---- frontmatter: typed keys (alias defaults to the key) ----
title:    string                       -- the feature name (also the H1)
status:   enum(planned, partial, implemented, wontfix)  -- lifecycle state
priority: int                          -- 1 (highest) .. 5
breaking: bool                         -- API-breaking change?
created:  date                         -- RFC3339 date
tags:     [string]+                    -- non-empty list of labels
owner?:   string                       -- optional assignee
spec_url? @spec: /^https?:\/\//        -- optional URL; alias renamed key -> "spec"
---

# @title /.+/                          -- required H1, any text (regex-labeled, level 1)

## Summary                             -- literal heading, NO @name -> auto-alias "summary"
  > @blurb                             -- a required non-empty paragraph (prose)

## @test_plan Test plan
  - [ ] @cases                         -- a checklist, required (bare = >=1)

## @manual Manual verification         -- heading nests headings
  ### @setup Setup
    - @items+                          -- bullet list, one or more
  ### @procedure Procedure
    1. @steps+                         -- ordered list, one or more
      - @note?                         -- list item nests an optional bullet
  ### @expect Expectations
    - [ ] @checks*                     -- checklist, zero or more (optional)

## @risks Risks
  - @items*                            -- bullet list, zero or more

## @signoff Sign-off                   -- bare literal labels -> scalar captures
  - @docs Docs:                        -- a bullet starting "Docs:"; value = text after it
  - @tests Tests:

## @decisions Decisions
  ### @entries+ /.+/                    -- repeated subsection: one or more, any title
    > @state /status:/i                -- a paragraph matching /status:/i
    - @points{1,5}                     -- 1..5 rationale bullets (explicit range)

## @refs? References                   -- optional heading (?)
  - @links* /^\[.+\]\(.+\)$/           -- regex-labeled bullets, zero or more
```

---

## 2. A conforming document

```markdown
---
title: Tab title follows OSC 0/2
status: implemented
priority: 2
breaking: false
created: 2026-06-20
tags: [osc, tabs, vte]
owner: dan
spec_url: https://example.com/osc
---

# Tab title follows OSC 0/2

## Summary
Updates the tab title from OSC 0/2 sequences, with UTF-8 validation.

## Test plan
- [x] OSC 2 with valid UTF-8 updates title — `tab::osc_title`
- [ ] Invalid UTF-8 payload is rejected

## Manual verification
### Setup
- external monitor at 150% scaling

### Procedure
1. Open a tab
2. Run the printf escape
   - note: use the staging build

### Expectations
- [ ] crisp glyphs

## Risks
- title cache may stale across profile switch

## Sign-off
- Docs: README updated
- Tests: covered

## Decisions
### Use OSC 2, not OSC 0
Status: accepted
- OSC 0 also sets the icon name, undesired
- narrower scope is safer

## References
- [OSC spec](https://example.com/osc)
```

`schema.validate(doc)` → `[]` (conforms).

---

## 3. Extracted data object

`schema.extract(doc)` → JSON keyed by aliases:

```json
{
  "title": "Tab title follows OSC 0/2",
  "status": "implemented",
  "priority": 2,
  "breaking": false,
  "created": "2026-06-20",
  "tags": ["osc", "tabs", "vte"],
  "owner": "dan",
  "spec": "https://example.com/osc",

  "summary": { "blurb": "Updates the tab title from OSC 0/2 sequences, with UTF-8 validation." },
  "test_plan": {
    "cases": ["OSC 2 with valid UTF-8 updates title — `tab::osc_title`", "Invalid UTF-8 payload is rejected"]
  },
  "manual": {
    "setup":     { "items": ["external monitor at 150% scaling"] },
    "procedure": { "steps": [ { "text": "Open a tab" },
                              { "text": "Run the printf escape", "note": "use the staging build" } ] },
    "expect":    { "checks": ["crisp glyphs"] }
  },
  "risks":   { "items": ["title cache may stale across profile switch"] },
  "signoff": { "docs": "README updated", "tests": "covered" },
  "decisions": {
    "entries": [
      { "title": "Use OSC 2, not OSC 0", "state": "Status: accepted",
        "points": ["OSC 0 also sets the icon name, undesired", "narrower scope is safer"] }
    ]
  },
  "refs": { "links": ["[OSC spec](https://example.com/osc)"] }
}
```

### Extraction conventions (worth pinning)

- A **scalar** capture (frontmatter field, `>` prose, labeled bullet) → its value.
  A **labeled bullet** (`- @docs Docs:`) captures the text *after* the label.
- An **unlabeled list** (`-`, `1.`, `- [ ]`) → an array. Items are plain strings
  unless the item has named children, in which case each item is an object with
  its lead text under `"text"` plus the child aliases (see `procedure.steps`).
- A **heading** with named children → an object of those children. A
  **variable-title** heading (regex/repeated, e.g. `### @entries+ /.+/`) also
  captures its heading text under `"title"`; a literal-title heading does not.
- **Single vs array** is decided by cardinality: bare/`?` ⇒ scalar-or-object,
  `+`/`*`/`{m,n}` ⇒ array.

---

## 4. Queries (jq, via `jaq`)

```
.status                          -> "implemented"
.manual.procedure.steps[1].note  -> "use the staging build"
.decisions.entries[].title       -> "Use OSC 2, not OSC 0"
.tags | length                   -> 3
```

Bare-alias addressing (unique alias, no path): `query(doc, "@blurb")` resolves to
`.summary.blurb`.

---

## 5. Edit in-place

```rust
schema.edit(doc, "manual.procedure.steps[1]", "Run `printf '\\033]2;hi\\007'`")?;
// or by unique alias:
schema.edit(doc, "owner", "alex")?;
```

Only the addressed node's source span is rewritten; every other byte — spacing,
the other steps, surrounding prose — is preserved.

---

## 6. Scaffold

`schema.scaffold()` (render with placeholders; `?`/`*` nodes omitted):

```markdown
---
title:
status: planned
priority:
breaking:
created:
tags: []
---

#

## Summary


## Test plan
- [ ]

## Manual verification
### Setup
-
### Procedure
1.
### Expectations

## Risks

## Sign-off
- Docs:
- Tests:

## Decisions
### 
Status:
-
```

---

## 7. Feature coverage map

| Feature | Demonstrated by |
|---|---|
| Directives `%ordered` / `%strict` / `%frontmatter` | top of §1 |
| Frontmatter `string` / `int` / `bool` / `date` | `title` / `priority` / `breaking` / `created` |
| Frontmatter `enum` / `[list]` / `/regex/` | `status` / `tags` / `spec_url` |
| Optional key `?` | `owner?`, `spec_url?` |
| Frontmatter alias override | `spec_url @spec` (key `spec_url` → alias `spec`) |
| Heading levels 1 / 2 / 3 | `# @title /.+/`, `## Summary`, `### @setup Setup` |
| Literal vs regex heading title | `## Summary` vs `# @title /.+/` |
| Repeated subsection | `### @entries+ /.+/` under Decisions |
| Optional heading `?` | `## @refs? References` |
| Bullet / ordered / checklist / prose | `-` / `1.` / `- [ ]` / `>` |
| Bare literal label (→ scalar) | `- @docs Docs:` |
| Regex label | `- @links* /^\[.+\]\(.+\)$/`, `> @state /status:/i` |
| Cardinality bare / `+` / `*` / `?` / `{m,n}` (glued) | `@cases` / `@items+` / `@checks*` / `@note?` / `@points{1,5}` |
| Nesting: heading→heading | Manual → Setup/Procedure/Expectations |
| Nesting: heading→list | Setup → bullets |
| Nesting: list-item→list | Procedure step → `note` |
| `@name` alias | throughout |
| Auto-derived alias | `## Summary` → `summary` |
| `--` description | every line |
