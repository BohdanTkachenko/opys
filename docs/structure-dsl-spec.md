# mdprism — design spec

*Markdown ⇄ data, via a template.*

**Status:** draft / under review · **Crate name:** `mdprism` *(chosen)*

A standalone Rust crate defining a **bidirectional mapping** between a markdown
document (YAML frontmatter + body) and a typed data object, from a single compact
schema. It can **validate**, **extract** (parse → data), **render** (data →
markdown), **scaffold**, **query**, and **edit in-place**. The schema *looks like
a skeleton of the document it describes*. `opys` is the first consumer (for
`kind = "structured"` section bodies), but the crate has no opys dependency and
is independently useful (runbooks, ADRs, postmortems, release notes…).

---

## 1. Capabilities

One schema defines a **bidirectional mapping** between a markdown document and a
typed data object (think *serde, for documents*). From it:

1. **Validate** — does a document conform? Located, descriptive errors.
2. **Extract** (parse) — markdown → typed **data object** (JSON), keyed by the
   schema's capture **names**.
3. **Render** (generate) — data object → markdown. Take a template (the schema) +
   variables (the data) and produce a conforming file.
4. **Scaffold** — `render` specialized to placeholder/default values: a starter
   document.
5. **Query** — jq-style selectors over the extracted object (a section, a nested
   node, a single list item).
6. **Edit in-place** — `render` specialized to *one* node: resolve a capture name
   or query → its source span → splice the new value, byte-accurately, leaving
   everything else untouched. *(The "LLM updates a list item with one command,
   100% accuracy" case — no hand-rolled `sed`, no full-file rewrite.)*

**Extract and render are inverses**: `extract(render(data)) == data`, and
`render(extract(md)) ≈ md` (modulo formatting normalization). Scaffold and edit
are just render restricted to placeholders / a single node. (2)–(6) build on two
foundational features baked in from day one: **scope-unique capture names** and
**source spans** on every node.

**Non-goals:** not a *general* markdown renderer (no markdown→HTML, no arbitrary
CommonMark transforms — it only renders *its own schema* from data); not a
programming language (only cardinality, regex labels, and jq queries); does not
own opys's reserved-key / relation / ID logic.

---

## 2. The DSL at a glance

A schema is a skeletal markdown file: an optional `--- … ---` frontmatter block
of typed keys, then a body skeleton of headings and typed (optionally nested)
lists. The `@name` alias sits **right after the marker**; a ` -- description`
trails the line.

```
---
title: string                        -- human title, also the H1
status: enum(planned, partial, implemented, wontfix)  -- lifecycle state
owner?: string
---

## @test_plan Test plan
  - [ ] @cases                       -- one checkbox per behavior

## @manual Manual verification
  ### @setup Setup                   -- preconditions for the run
    - @items+
  ### @procedure Procedure
    1. @steps+                       -- ordered, reproducible steps
      - @note?                       -- optional note under a step
  ### @expect Expectations
    - [ ] @checks+
```

The same text — cardinality stripped, placeholders filled — is the **scaffold**
for a new document (§8). The `@names` define the shape of the **extracted data
object** (§6) and the addressing for **queries and edits** (§7).

---

## 3. Grammar

### 3.1 Overall shape

```ebnf
schema      = directive* frontmatter? body
directive   = "%" key "=" value NEWLINE         # schema-level options (§5)
frontmatter = "---" NEWLINE fm-field* "---" NEWLINE
body        = node*
```

### 3.2 Frontmatter schema

```ebnf
fm-field = key "?"? ("@" name)? ":" SP fm-type (" -- " desc)? NEWLINE
fm-type  = "string" | "int" | "bool" | "date"
         | "enum(" ident ("," SP? ident)* ")"
         | "[" fm-type "]"          # list of T
         | "/" regex "/"            # string matching regex
```

- `key?` ⇒ optional key; otherwise required.
- The alias **defaults to the key**; an explicit `@name` (right after the key)
  renames it (`title @display: string`).
- Frontmatter is **closed by default** (unknown keys are errors); `%frontmatter=open` allows extras.

### 3.3 Body structure

Each node is one line — `marker @name? label? card? -- desc?` — plus an
optionally indented child block. The `@name` alias comes **immediately after the
marker**, before the label/heading text; the description trails. Indentation
(normalized) encodes nesting.

```ebnf
node     = INDENT marker ("@" name)? label? card? (" -- " desc)? NEWLINE children?
children = (node, indented one level deeper)+

marker   = heading | bullet | ordered | checkbox | prose
heading  = "#"{1,6} SP                # level = count of '#'; title is the label
bullet   = "-"
ordered  = digits "."                # "1."
checkbox = "- [ ]"
prose    = ">"                        # a non-empty paragraph

label    = '"' literal '"'            # item/heading text starts with literal
         | "/" regex "/"              # …or matches regex
         | text                       # a bare literal heading title
card     = "+" | "*" | "?" | "{" int ("," int?)? "}"
```

**Cardinality** (item count on lists; presence on headings/prose):

| Suffix | Meaning |
|---|---|
| (none) | required (a list ⇒ ≥1 item) |
| `+` / `*` / `?` | ≥1 / ≥0 / optional |
| `{m}` `{m,}` `{m,n}` | explicit bounds |

`## @entries /.+/+` = one or more headings at that level (repeated subsection). A
child block under a **list** constrains *each item*.

**Annotations**: `@name` (right after the marker) is a capture **alias**
(`[a-z][a-z0-9_]*`), **unique within its scope**, attachable to *any* block
(heading, list, item, prose, frontmatter field). The alias — not the heading
text — is what selectors, extraction, and edits address, so it is
**rename-proof**: changing a heading's displayed text never breaks consumers. A
heading without an explicit `@name` gets an **auto-derived alias** = the slug of
its title (`Manual verification` → `manual_verification`); an explicit `@name`
overrides it. ` -- text` (trailing) is a description.

### 3.4 Markdown coverage — does it support "full markdown"?

Two layers, kept separate:

- **The document** may use the **full** CommonMark + GFM syntax — tables,
  fenced code, blockquotes, emphasis, links, images, HTML, footnotes, etc. The
  parser (§9, comrak) reads all of it; mdprism never restricts what a document
  contains.
- **The schema vocabulary** models *block structure*: headings, the three list
  kinds, and paragraphs (`>`). That is what you can currently *assert and
  capture*. **Inline** content inside a captured node (bold, links, inline code)
  is preserved as raw markdown text — constrain it with a `/regex/` label, not a
  sub-grammar.

Block types the schema does **not** yet have markers for — tables, fenced code,
blockquotes, thematic breaks, images, footnotes, raw HTML — are **opaque**: under
`%strict=true` they must sit inside a region the schema allows as freeform (see
the `*` wildcard below), and under `%strict=false` they're simply ignored.

**Planned vocabulary extensions** (post-v0), so the schema can assert richer
blocks:

| Marker (proposed) | Asserts |
|---|---|
| `\| col \| col \|` | a GFM table with these columns |
| ` ```lang ` | a fenced code block (optional language) |
| `>> ` | a blockquote (its own child block) |
| `*` / `*+` | a freeform wildcard — *any* block(s) here (the escape hatch that lets `%strict` schemas permit rich prose) |

Until then, model rich/freeform regions with `>` (paragraph) or a future `*`
wildcard; everything the document contains still round-trips through render/edit
because spans are byte-preserved (§7).

| Behavior | Default | Override |
|---|---|---|
| **Ordering** | **strict** — body nodes must appear in declared order | `%ordered=false` (any order) |
| **Strictness** | **error on mismatch / extras** | per-node `?`/`*`, or `%strict=false` (extras allowed) |
| **Frontmatter** | **closed** — unknown keys error | `%frontmatter=open` |
| **Markdown parser** | **established library** (see §9) | — |

`%`-directives at the top of a schema set these per-schema; the host (opys / API)
can also set them programmatically.

---

## 5. Descriptions → richer everything

A node's ` -- description` is used to:

- **Enrich errors:** `Procedure › steps: expected ≥1 ordered item — "ordered,
  reproducible steps"`.
- **Power IDE integration** (future): hover text, completion docs for schema
  authors and document authors.
- **Document the schema** itself (it reads as annotated structure).

Descriptions are optional and never affect matching.

---

## 6. The data model: validate → typed object

Parsing a **conforming** document against the schema yields a JSON-like value,
keyed by capture `@names`, ready for `serde_json` consumers and jq queries.

- A named **heading** → an object of its named children; a *variable-title*
  heading (regex/repeated) also captures its heading text under `"title"`.
- A named **list** → an array; items are plain strings unless they have named
  children, in which case each item is an object with its lead text under
  `"text"` plus the child aliases.
- A named **scalar** (prose, frontmatter field, labeled bullet — which captures
  the text *after* the label) → a typed value (string / int / bool / date /
  enum). Single-vs-array follows cardinality: bare/`?` ⇒ scalar, `+`/`*`/`{m,n}` ⇒ array.
- Unnamed nodes are validated but not captured (headings auto-derive an alias —
  see §3.3 — so they remain addressable).

A complete worked example of every construct, with the resulting object, lives in
[`mdprism-reference.md`](./mdprism-reference.md).

**Example** (schema from §2) →

```json
{
  "title": "Tab title follows OSC",
  "status": "implemented",
  "manual": {
    "setup":     { "items": ["external monitor at 150%"] },
    "procedure": { "steps": ["Open a tab", "Run the command"] },
    "expect":    { "checks": ["crisp glyphs", "no clipping"] }
  }
}
```

Internal types (sketch):

```rust
pub struct Schema { pub opts: SchemaOpts, pub frontmatter: Vec<FieldSchema>, pub body: Vec<Node> }

pub enum Node {
    Heading { level: u8, title: Match, card: Card, name: Option<Name>, desc: Option<String>, children: Vec<Node> },
    List    { style: ListStyle, item: Option<Match>, card: Card, name: Option<Name>, desc: Option<String>, children: Vec<Node> },
    Prose   { text: Option<Match>, card: Card, name: Option<Name>, desc: Option<String> },
}
pub enum ListStyle { Bullet, Ordered, Checklist }
pub enum Match { Literal(String), Regex(Regex) }
pub enum Card  { Required, Optional, Star, Plus, Range(u32, Option<u32>) }

/// A captured value with its source span, for extraction AND editing.
pub struct Captured { pub value: serde_json::Value, pub span: Span }
```

---

## 7. Query & edit (jq-style)

Once a document is parsed to the JSON-like value **and** every captured node
carries its source `Span`, two things fall out:

- **Query:** evaluate a jq selector against the value (e.g. `.manual.procedure.steps[1]`).
  We use an existing Rust jq engine (`jaq`) rather than inventing a query syntax.
- **Edit in-place:** resolve a capture name or jq path → the node's `Span` in the
  original source → splice a new value, re-rendering only that node. Everything
  else (formatting, surrounding prose, other items) is byte-preserved.

This requires the markdown parser to provide **source positions** (§9), which is
a hard requirement on the parser choice.

**Addressing by alias.** Beyond jq paths, `edit`/`query` accept a bare alias
(`steps`) when it is unique across the schema, resolving to the full path
internally; ambiguous aliases require the dotted path (`manual.procedure.steps`).
The crate maintains a name→node index for this. So consumers get short, stable
handles that survive heading renames — the whole point of aliases.

**Custom extraction templates (future, host-exposed):** because captures are
named and queryable, a consumer can ship its own schemas purely to *extract*
data — "give me `.manual.steps` from every doc" — without that schema being the
validation schema. opys (and other consumers) can expose this so users define
their own extraction templates per use case.

---

## 8. Scaffolding

`Schema::scaffold()` walks the tree: frontmatter keys with placeholder values,
headings verbatim, one placeholder item per required list, labels as literal
prefixes, `?`/`*` nodes omitted. Descriptions may be emitted as guiding
comments. The schema and the new-document template are one artifact.

---

## 9. Markdown parsing — decided: established library

A real parser is required (nesting, lists, headings) **and it must expose source
positions** for the in-place-edit feature (§7).

- **Recommended: `comrak`** — CommonMark + GFM (task lists, tables), a real AST,
  and a `sourcepos` extension giving line/col spans per node. Best fit for the
  tree + spans we need.
- **Alternative: `pulldown-cmark`** — fast, byte **offsets** via
  `into_offset_iter`, but event-stream (we'd rebuild a tree).

Decision: use a well-established library (per direction). Spec assumes **comrak**
for its AST + sourcepos unless changed. Note: this pulls comrak (and `jaq` for
queries) into the dependency tree of any consumer, including opys.

---

## 10. Public API (sketch)

```rust
pub fn parse_schema(src: &str) -> Result<Schema, SchemaError>;

impl Schema {
    pub fn validate(&self, md: &str) -> Vec<Problem>;
    /// Parse a conforming doc into the typed object (errors if non-conforming).
    pub fn extract(&self, md: &str) -> Result<serde_json::Value, Vec<Problem>>;
    /// Render a data object into a conforming markdown document (inverse of extract).
    pub fn render(&self, data: &serde_json::Value) -> Result<String, RenderError>;
    /// Render with placeholder/default values — a starter document.
    pub fn scaffold(&self) -> String;
    /// jq selector over the extracted object.
    pub fn query(&self, md: &str, jq: &str) -> Result<Vec<serde_json::Value>, QueryError>;
    /// Replace the node addressed by a capture name or jq path; returns new source.
    pub fn edit(&self, md: &str, target: &str, value: &str) -> Result<String, EditError>;
}

pub struct Problem    { pub path: Vec<String>, pub message: String, pub span: Option<Span> }
pub struct SchemaError{ pub line: usize, pub col: usize, pub message: String }
```

---

## 11. opys integration (body-structure-only)

opys keeps its own frontmatter/field validation and reserved-key/relation/ID
logic; it uses this crate **only** for `kind = "structured"` section bodies.

```toml
[[types.feature.sections]]
heading = "Manual verification"
kind = "structured"
structure = '''
### Setup
  -+
### Procedure
  1.+
'''
```

- `opys verify` extracts the section body and calls `validate`; problems prefixed
  with doc id + heading.
- `opys new` calls `scaffold()` for the section body.
- The flat `[[parts]]` model is **dropped**.
- Later, opys can expose `query`/`edit` (e.g. `opys edit FEAT-1 --in "Manual
  verification" --set procedure.steps[1] "…"`) — the precise-edit use case.

---

## 12. Open decisions

1. **Exact annotation syntax** — `@name` + ` -- description` proposed; confirm.
2. **Query engine** — `jaq` (jq in Rust) proposed.
3. **Markdown library** — `comrak` (AST + sourcepos) proposed; `pulldown-cmark` alt.

(Crate name **`mdprism`** is **resolved**.)

(Parser-is-a-library, strict ordering, strict matching, closed frontmatter are
**resolved** per §4.)

---

## 13. Phasing

1. Workspace restructure: opys → Cargo workspace; add the crate member.
2. **v0 — validate + scaffold:** frontmatter + body grammar, `@name`/`--desc`
   parsed and carried, spans tracked, errors. Wire opys `structured` sections;
   delete `[[parts]]`.
3. **Extract:** schema → typed JSON object via capture names.
4. **Render:** data object → markdown (inverse of extract); scaffold becomes a
   thin wrapper over it with placeholder data.
5. **Query:** `jaq` selectors over the object.
6. **Edit in-place:** name/path → span → byte-accurate splice; expose via opys.
7. Docs + (optional) standalone publish + IDE integration groundwork.
