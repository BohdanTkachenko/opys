//! The parsed schema data model — the in-memory form of the DSL.
//!
//! See `docs/structure-dsl-spec.md` for the language this represents.

/// A parsed schema: per-schema options, the frontmatter field schemas, and the
/// body node tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Schema {
    pub opts: SchemaOpts,
    pub frontmatter: Vec<FieldSchema>,
    pub body: Vec<Node>,
}

/// The `%`-directive options. Defaults match the spec: strict ordering, strict
/// matching, closed frontmatter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaOpts {
    /// Body nodes must appear in declared order (`%ordered`).
    pub ordered: bool,
    /// Error on mismatch / unexpected blocks (`%strict`).
    pub strict: bool,
    /// Allow undeclared frontmatter keys (`%frontmatter = open`).
    pub frontmatter_open: bool,
}

impl Default for SchemaOpts {
    fn default() -> Self {
        SchemaOpts {
            ordered: true,
            strict: true,
            frontmatter_open: false,
        }
    }
}

/// One typed frontmatter key.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldSchema {
    pub key: String,
    /// Capture alias; defaults to `key` unless renamed with `@name`.
    pub alias: String,
    pub optional: bool,
    pub ty: FieldType,
    pub desc: Option<String>,
}

/// A frontmatter value type. `Regex` keeps the pattern *source* so the type is
/// `PartialEq`/`Clone`; compile it on demand.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldType {
    Str,
    Int,
    Bool,
    Date,
    Enum(Vec<String>),
    List(Box<FieldType>),
    Regex(String),
}

/// A body node: a heading (with children), a list (with a per-item child
/// schema), or a required paragraph.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Heading {
        level: u8,
        title: Match,
        head: Head,
        children: Vec<Node>,
    },
    List {
        style: ListStyle,
        item: Option<Match>,
        head: Head,
        children: Vec<Node>,
    },
    Prose {
        text: Option<Match>,
        head: Head,
    },
}

impl Node {
    pub(crate) fn set_children(&mut self, kids: Vec<Node>) {
        match self {
            Node::Heading { children, .. } | Node::List { children, .. } => *children = kids,
            // Prose has no children; ignore (the parser rejects indented prose).
            Node::Prose { .. } => {}
        }
    }
}

/// The shared annotation head of every node: alias, cardinality, description.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Head {
    /// Explicit `@name`. `None` means "auto-derive" (headings slug their title).
    pub name: Option<String>,
    pub card: Card,
    pub desc: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListStyle {
    Bullet,
    Ordered,
    Checklist,
}

/// A label: a literal the text must start with, or a regex (pattern source) the
/// text must match. A bare heading title is a `Literal`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Match {
    Literal(String),
    Regex(String),
}

/// Cardinality. On a list it bounds item count; on a heading/prose it is
/// presence. `Required` is the bare default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Card {
    #[default]
    Required,
    Optional,
    Star,
    Plus,
    Range(u32, Option<u32>),
}
