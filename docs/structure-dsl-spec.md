# Markdown Structure DSL — design spec

**Status:** draft / under review · **Working crate name:** `mdrubric` *(TBD — `md*` name, see naming)*

A standalone Rust crate that validates a **full markdown document** (YAML
frontmatter + body) against a schema written in a compact, indentation-based
DSL. The schema *looks like a skeleton of the document it validates*. `opys` is
the first consumer, but the crate has no opys dependency and is independently
useful (runbooks, ADRs, postmortems, release notes…).

---

## 1. Goals / non-goals

**Goals**

- One textual schema describes a whole file: typed frontmatter + structured body.
- The schema is readable as a stripped-down example of a conforming document.
- A schema doubles as a **scaffold** template for new documents.
- Precise, located errors for both a malformed *schema* and a non-conforming
  *document*.
- Pure Rust, MSRV 1.88, minimal dependencies.

**Non-goals**

- Not a general markdown renderer or transformer; validation + scaffolding only.
- Not a programming language — no variables, conditionals, or expressions beyond
  cardinality and regex matches.
- Does **not** own opys's reserved-key / relation / ID logic. opys uses this crate
  only for `kind = "structured"` section bodies (see §10).

---

## 2. The DSL at a glance

A schema is a skeletal markdown file: an optional `--- … ---` frontmatter block
of typed keys, then a body skeleton of headings and typed (optionally nested)
lists.

```
---
title: string
status: enum(planned, partial, implemented, wontfix)
tags: [string]+              # non-empty list of strings
owner?: string               # optional key
spec?: /^https?:\/\//        # optional, must match this regex
created: date
---

## Test plan
  - [ ]+                     # a checklist, >= 1 item

## Manual verification
  ### Setup
    -+                       # a bullet list, >= 1 item
  ### Procedure
    1.+                      # an ordered list, >= 1 item
      -?                     # each step MAY carry a nested bullet list
```

The same text, with cardinality stripped and placeholders filled, is the
scaffold an authoring tool emits for a new document (§9).

---

## 3. Grammar

### 3.1 Overall shape

```ebnf
schema       = frontmatter? body
frontmatter  = "---" NEWLINE fm-field* "---" NEWLINE
body         = node*
```

### 3.2 Frontmatter schema

```ebnf
fm-field = key "?"? ":" SP fm-type NEWLINE
fm-type  = "string" | "int" | "bool" | "date"
         | "enum(" ident ("," SP? ident)* ")"
         | "[" fm-type "]"          # a list of T
         | "/" regex "/"            # a string matching regex
```

- `key?` marks an **optional** key; otherwise required.
- A `[T]` list may carry a cardinality suffix (`[string]+` = non-empty).
- Frontmatter is **open by default** (unknown keys allowed); a schema-level
  `strict` flag (API option) rejects undeclared keys.

### 3.3 Body structure

Each node is one line — `marker label? card?` — plus an optionally indented
child block. Indentation (2 spaces per level, normalized) encodes nesting.

```ebnf
node     = INDENT marker label? card? NEWLINE children?
children = (node, indented one level deeper)+

marker   = heading | bullet | ordered | checkbox | prose
heading  = "#"{1,6} SP text          # level = count of '#'
bullet   = "-"
ordered  = digits "."                # "1."
checkbox = "- [ ]"
prose    = ">"                        # a required non-empty paragraph

label    = '"' literal '"'            # item/heading text starts with literal
         | "/" regex "/"              # …or matches regex
card     = "+" | "*" | "?" | "{" int ("," int?)? "}"
```

**Marker meaning**

| Schema line | Asserts in the document |
|---|---|
| `## Title` / `### Title` | a heading of that level with that title; its child block follows under it |
| `## /.+/+` | one or more headings at that level (any title) — a *repeated subsection* |
| `-` | a bullet (unordered) list |
| `1.` | an ordered list |
| `- [ ]` | a checklist (GFM task list) |
| `> ` | a required non-empty paragraph |
| trailing `"…"` / `/…/` | the item/heading text must start-with / match it |

**Cardinality** applies to *item count* on lists and *presence* on
headings/prose:

| Suffix | Meaning |
|---|---|
| (none) | required (a list ⇒ ≥1 item) |
| `+` | one or more |
| `*` | zero or more (i.e. optional list) |
| `?` | optional (0 or 1) |
| `{m}` `{m,}` `{m,n}` | explicit bounds |

A child block under a **list** node constrains *each item* of that list (this is
how nesting works — a list inside a list item, a labeled sub-bullet, etc.).

---

## 4. Semantics

### 4.1 Frontmatter

Parse the document's YAML frontmatter into a map. For each declared field:
required-but-absent ⇒ error; present ⇒ type-check (`int`/`bool`/`date` parse,
`enum` membership, `[T]` element types, `/regex/` match). Unknown keys are
ignored unless `strict`.

### 4.2 Body — tree-pattern matching

1. Parse the document body into a **block tree** (headings by level, lists by
   marker + indentation, list items, paragraphs).
2. Walk the schema tree against the document tree. For each schema node, find the
   matching document block(s); check cardinality; recurse into children.
3. Each unmet **required** node yields one `Problem` with a breadcrumb path.

**Ordering** — default is **order-independent presence**: declared subsections
may appear in any order; all required ones must be present. An opt-in `ordered`
flag (API/schema directive) enforces declared order.

**Strictness** — default is **schema-as-minimum**: the document may contain extra
prose, lists, or headings the schema doesn't mention. An opt-in `strict` flag
flags unexpected blocks.

**List cardinality** counts items; **nesting** recurses the child schema into
each item's nested blocks. Optionals (`?`/`*`) use greedy matching with limited
backtracking; because nodes are keyed by heading title and list style,
ambiguity is low in practice.

**Label match** — `"Setup:"` ⇒ the item/heading text **starts with** `Setup:`;
`/…/ ` ⇒ regex match anywhere in the text.

---

## 5. Data model (Rust)

```rust
pub struct Schema {
    pub frontmatter: Vec<FieldSchema>,   // empty if no fence
    pub body: Vec<Node>,
    pub opts: SchemaOpts,                // ordered, strict, …
}

pub struct FieldSchema { pub key: String, pub optional: bool, pub ty: FieldType }
pub enum FieldType { Str, Int, Bool, Date, Enum(Vec<String>), List(Box<FieldType>), Regex(Regex) }

pub enum Node {
    Heading  { level: u8, title: Match, card: Card, children: Vec<Node> },
    List     { style: ListStyle, item: Option<Match>, card: Card, children: Vec<Node> },
    Prose    { text: Option<Match>, card: Card },
}
pub enum ListStyle { Bullet, Ordered, Checklist }
pub enum Match { Literal(String), Regex(Regex) }
pub enum Card  { Required, Optional, Star, Plus, Range(u32, Option<u32>) }
```

A separate, internal `Document` block-tree mirrors `Node` for the matcher.

---

## 6. Public API (sketch)

```rust
/// Parse DSL source into a schema (errors carry line:col).
pub fn parse_schema(src: &str) -> Result<Schema, SchemaError>;

impl Schema {
    /// Validate a full markdown document; empty Vec == conforms.
    pub fn validate(&self, markdown: &str) -> Vec<Problem>;
    /// Emit a starter document conforming to this schema.
    pub fn scaffold(&self) -> String;
}

pub struct Problem { pub path: Vec<String>, pub message: String, pub span: Option<Span> }
pub struct SchemaError { pub line: usize, pub col: usize, pub message: String }
```

---

## 7. Errors

Two surfaces, both located:

- **Schema parse** (`SchemaError`): `line 4:3: '1.' cannot nest under a checklist`.
- **Document validation** (`Problem`, breadcrumb path):
  - `Manual verification › Procedure: expected an ordered list with ≥1 item`
  - `Manual verification › Setup › item 2: missing "Expect:" bullet`
  - `frontmatter: 'status' must be one of: planned, partial, implemented, wontfix`

---

## 8. Markdown parsing — the one real engineering decision

Nesting/lists/headings need a real block parse (today's opys parsing is flat
regex over lines).

- **Recommended:** a small hand-rolled block parser (headings by `^#{1,6} `,
  lists by marker + indent depth, paragraphs). No dependency, matches opys's
  ethos, full control over spans for error reporting. ~200 LOC.
- **Alternative:** depend on `pulldown-cmark` for a CommonMark AST. More robust
  for edge cases, but a heavier dependency and a departure from hand-rolled
  parsing.

**Decision needed.** Spec assumes the hand-rolled parser unless changed.

---

## 9. Scaffolding

`Schema::scaffold()` walks the tree and emits a conforming skeleton: the
frontmatter keys with placeholder values, headings verbatim, one placeholder
item per required list, labels as literal prefixes; `?`/`*` nodes omitted. The
schema and the new-document template are one artifact.

---

## 10. opys integration (body-structure-only)

Per decision: opys keeps its own frontmatter/field validation and reserved-key /
relation / ID logic. It uses this crate **only** for `kind = "structured"`
section bodies.

- A `structured` section in `opys.toml` carries a schema string (the body
  portion of the DSL — no frontmatter fence), e.g.

  ```toml
  [[types.feature.sections]]
  heading = "Manual verification"
  kind = "structured"
  structure = '''
  ### Setup
    -+
  ### Procedure
    1.+
  ### Expectations
    - [ ]+
  '''
  ```

- `opys verify` extracts the `## <heading>` section body and calls
  `Schema::validate`; problems are prefixed with the doc id + heading.
- `opys new` calls `Schema::scaffold()` for the section body.
- The flat `[[parts]]` model is **dropped** in favour of this.

---

## 11. Open decisions to confirm

1. **Crate name** (see naming discussion).
2. **Markdown parser**: hand-rolled (recommended) vs `pulldown-cmark`.
3. **Default ordering**: order-independent presence (recommended) vs strict order.
4. **Default strictness**: schema-as-minimum (recommended) vs reject-extras.
5. **Frontmatter open vs closed** by default (recommended open; opys won't use it).

---

## 12. Phasing

1. Workspace restructure: opys becomes a Cargo workspace; add the crate member.
2. Crate v0: frontmatter schema + flat body (headings + lists, no nesting) +
   validate + errors + tests.
3. Nesting + cardinality + labels + scaffolding.
4. Wire opys `structured` sections to it; delete `[[parts]]`.
5. Docs + (optional) standalone publish.

---

## 13. Worked example

**Schema**

```
---
title: string
owner?: string
---
## Steps
  1.+
## Risks
  -*
```

**Conforming document**

```markdown
---
title: Migrate auth
---
# Migrate auth
## Steps
1. Snapshot the DB
2. Flip the flag
## Risks
- token cache may stale
```

**Result:** `[]` (conforms — `owner` optional, `Risks` may be empty, order of
Steps/Risks fixed only if `ordered`).
