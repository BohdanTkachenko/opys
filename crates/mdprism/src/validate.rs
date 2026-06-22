//! Validation: does a markdown document conform to the schema's body?
//!
//! The document is parsed with comrak, lowered into a simplified block tree
//! (headings reconstructed into nested sections), then matched against the
//! schema body. Frontmatter validation is a later phase; the frontmatter block
//! is stripped before parsing.

use crate::error::Problem;
use crate::schema::*;
use comrak::nodes::{AstNode, ListType, NodeValue};
use comrak::{parse_document, Arena, Options};
use regex::Regex;
use serde_json::{Map, Value};

impl Schema {
    /// Validate a markdown document's **body** against the schema. An empty
    /// result means it conforms. (Frontmatter typing: future phase.)
    pub fn validate(&self, markdown: &str) -> Vec<Problem> {
        let body = strip_frontmatter(markdown);
        let arena = Arena::new();
        let root = parse_document(&arena, body, &Options::default());
        let doc = build_blocks(root.children());

        let mut problems = Vec::new();
        let mut path = Vec::new();
        match_nodes(&self.body, &doc, self.opts, &mut path, &mut problems);
        problems
    }

    /// Parse a **conforming** document into the typed data object, keyed by the
    /// schema's capture aliases. Returns the validation problems if it does not
    /// conform. (Frontmatter capture: future phase — body only for now.)
    pub fn extract(&self, markdown: &str) -> Result<Value, Vec<Problem>> {
        let problems = self.validate(markdown);
        if !problems.is_empty() {
            return Err(problems);
        }
        let body = strip_frontmatter(markdown);
        let arena = Arena::new();
        let root = parse_document(&arena, body, &Options::default());
        let doc = build_blocks(root.children());

        let mut obj = Map::new();
        extract_into(&self.body, &doc, &mut obj);
        Ok(Value::Object(obj))
    }
}

// ---- a simplified document block tree ------------------------------------

#[derive(Debug, Clone)]
enum DocBlock {
    Section {
        level: u8,
        title: String,
        children: Vec<DocBlock>,
    },
    List {
        ordered: bool,
        items: Vec<DocItem>,
    },
    Para(String),
}

#[derive(Debug, Clone)]
struct DocItem {
    text: String,
    checked: Option<bool>,
    children: Vec<DocBlock>,
}

/// One flat block before heading-nesting is reconstructed.
enum Flat {
    Heading(u8, String),
    Block(DocBlock),
}

fn build_blocks<'a>(nodes: impl Iterator<Item = &'a AstNode<'a>>) -> Vec<DocBlock> {
    let flats: Vec<Flat> = nodes.filter_map(flat_of).collect();
    let mut pos = 0;
    nest(&flats, &mut pos, 0)
}

fn flat_of<'a>(node: &'a AstNode<'a>) -> Option<Flat> {
    match &node.data.borrow().value {
        NodeValue::Heading(h) => Some(Flat::Heading(h.level, text_of(node))),
        NodeValue::List(nl) => Some(Flat::Block(doc_list(node, nl.list_type))),
        NodeValue::Paragraph => Some(Flat::Block(DocBlock::Para(text_of(node)))),
        _ => None,
    }
}

/// Reconstruct heading nesting: a heading owns following blocks and deeper
/// headings until a heading of the same or higher rank.
fn nest(flats: &[Flat], pos: &mut usize, parent_level: usize) -> Vec<DocBlock> {
    let mut out = Vec::new();
    while *pos < flats.len() {
        match &flats[*pos] {
            Flat::Heading(level, title) => {
                if (*level as usize) <= parent_level {
                    break;
                }
                let (level, title) = (*level, title.clone());
                *pos += 1;
                let children = nest(flats, pos, level as usize);
                out.push(DocBlock::Section {
                    level,
                    title,
                    children,
                });
            }
            Flat::Block(b) => {
                out.push(b.clone());
                *pos += 1;
            }
        }
    }
    out
}

fn doc_list<'a>(list: &'a AstNode<'a>, list_type: ListType) -> DocBlock {
    let mut items = Vec::new();
    for item in list.children() {
        let mut text = String::new();
        let mut child_nodes = Vec::new();
        for ch in item.children() {
            match &ch.data.borrow().value {
                NodeValue::Paragraph if text.is_empty() => text = text_of(ch),
                NodeValue::List(_) => child_nodes.push(ch),
                _ => {}
            }
        }
        let (checked, text) = strip_checkbox(&text);
        items.push(DocItem {
            text,
            checked,
            children: build_blocks(child_nodes.into_iter()),
        });
    }
    DocBlock::List {
        ordered: matches!(list_type, ListType::Ordered),
        items,
    }
}

/// Concatenate the inline text of a node (headings, paragraphs, item lines).
fn text_of<'a>(node: &'a AstNode<'a>) -> String {
    let mut s = String::new();
    collect_text(node, &mut s);
    s.trim().to_string()
}

fn collect_text<'a>(node: &'a AstNode<'a>, out: &mut String) {
    for c in node.children() {
        match &c.data.borrow().value {
            NodeValue::Text(t) => out.push_str(t),
            NodeValue::Code(code) => out.push_str(&code.literal),
            NodeValue::SoftBreak | NodeValue::LineBreak => out.push(' '),
            _ => collect_text(c, out),
        }
    }
}

/// Pull a leading `[ ]` / `[x]` checkbox off an item's text (no GFM extension).
fn strip_checkbox(text: &str) -> (Option<bool>, String) {
    let t = text.trim_start();
    if let Some(rest) = t.strip_prefix("[ ] ") {
        (Some(false), rest.to_string())
    } else if let Some(rest) = t.strip_prefix("[x] ").or_else(|| t.strip_prefix("[X] ")) {
        (Some(true), rest.to_string())
    } else {
        (None, text.to_string())
    }
}

fn strip_frontmatter(md: &str) -> &str {
    let Some(rest) = md.strip_prefix("---\n") else {
        return md;
    };
    match rest.find("\n---") {
        Some(end) => {
            let after = &rest[end + 4..];
            after.strip_prefix('\n').unwrap_or(after)
        }
        None => md,
    }
}

// ---- the matcher ----------------------------------------------------------

fn match_nodes(
    schema: &[Node],
    doc: &[DocBlock],
    opts: SchemaOpts,
    path: &mut Vec<String>,
    problems: &mut Vec<Problem>,
) {
    let mut cursor = 0usize;
    for node in schema {
        let start = if opts.ordered { cursor } else { 0 };
        match node {
            Node::Heading {
                level,
                title,
                head,
                children,
            } => {
                let idxs: Vec<usize> = (start..doc.len())
                    .filter(|&i| is_section(&doc[i], *level, title))
                    .collect();
                if let Some(msg) = card_problem(head.card, idxs.len(), "section") {
                    problems.push(problem(path, alias(node), msg));
                }
                for &i in &idxs {
                    if let DocBlock::Section { children: sc, .. } = &doc[i] {
                        path.push(alias(node));
                        match_nodes(children, sc, opts, path, problems);
                        path.pop();
                    }
                }
                if opts.ordered {
                    if let Some(&last) = idxs.last() {
                        cursor = last + 1;
                    }
                }
            }
            Node::List {
                style,
                item,
                head,
                children,
            } => {
                let found = (start..doc.len()).find(|&i| is_list(&doc[i], *style));
                let count = match found {
                    Some(i) => {
                        if let DocBlock::List { items, .. } = &doc[i] {
                            items.len()
                        } else {
                            0
                        }
                    }
                    None => 0,
                };
                if let Some(msg) = card_problem(head.card, count, "item") {
                    problems.push(problem(path, alias(node), msg));
                }
                if let Some(i) = found {
                    if let DocBlock::List { items, .. } = &doc[i] {
                        for it in items {
                            if let Some(m) = item {
                                if !matches_text(m, &it.text) {
                                    problems.push(problem(
                                        path,
                                        alias(node),
                                        format!("item does not match {}: {}", describe(m), it.text),
                                    ));
                                }
                            }
                            if !children.is_empty() {
                                path.push(alias(node));
                                match_nodes(children, &it.children, opts, path, problems);
                                path.pop();
                            }
                        }
                    }
                    if opts.ordered {
                        cursor = i + 1;
                    }
                }
            }
            Node::Prose { text, head } => {
                let found = (start..doc.len()).find(|&i| matches!(doc[i], DocBlock::Para(_)));
                if let Some(i) = found {
                    if let DocBlock::Para(t) = &doc[i] {
                        if let Some(m) = text {
                            if !matches_text(m, t) {
                                problems.push(problem(
                                    path,
                                    alias(node),
                                    format!("paragraph does not match {}", describe(m)),
                                ));
                            }
                        }
                    }
                    if opts.ordered {
                        cursor = i + 1;
                    }
                } else if presence_required(head.card) {
                    problems.push(problem(
                        path,
                        alias(node),
                        "missing required paragraph".into(),
                    ));
                }
            }
        }
    }
}

fn is_section(b: &DocBlock, level: u8, title: &Match) -> bool {
    matches!(b, DocBlock::Section { level: l, title: t, .. } if *l == level && matches_text(title, t))
}

fn is_list(b: &DocBlock, style: ListStyle) -> bool {
    let DocBlock::List { ordered, items } = b else {
        return false;
    };
    match style {
        ListStyle::Ordered => *ordered,
        ListStyle::Bullet => !*ordered,
        ListStyle::Checklist => !*ordered && items.iter().any(|i| i.checked.is_some()),
    }
}

fn matches_text(m: &Match, text: &str) -> bool {
    match m {
        Match::Literal(l) => text.trim_start().starts_with(l),
        Match::Regex(p) => Regex::new(p).map(|re| re.is_match(text)).unwrap_or(false),
    }
}

fn describe(m: &Match) -> String {
    match m {
        Match::Literal(l) => format!("\"{l}\""),
        Match::Regex(p) => format!("/{p}/"),
    }
}

/// `Some(message)` when `count` violates the cardinality. `unit` is "section"
/// or "item" for the message.
fn card_problem(card: Card, count: usize, unit: &str) -> Option<String> {
    let ok = match card {
        Card::Required | Card::Plus => count >= 1,
        Card::Optional => count <= 1,
        Card::Star => true,
        Card::Range(min, max) => {
            count >= min as usize && max.map(|m| count <= m as usize).unwrap_or(true)
        }
    };
    if ok {
        return None;
    }
    Some(match card {
        Card::Required | Card::Plus => format!("expected at least one {unit}, found {count}"),
        Card::Optional => format!("expected at most one {unit}, found {count}"),
        Card::Range(min, Some(max)) => format!("expected {min}..{max} {unit}(s), found {count}"),
        Card::Range(min, None) => format!("expected at least {min} {unit}(s), found {count}"),
        Card::Star => unreachable!(),
    })
}

fn presence_required(card: Card) -> bool {
    matches!(card, Card::Required | Card::Plus | Card::Range(1.., _))
}

fn alias(node: &Node) -> String {
    let head = match node {
        Node::Heading { head, title, .. } => {
            if let Some(n) = &head.name {
                return n.clone();
            }
            if let Match::Literal(t) = title {
                return slug(t);
            }
            head
        }
        Node::List { head, .. } | Node::Prose { head, .. } => head,
    };
    head.name.clone().unwrap_or_else(|| "block".to_string())
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

fn problem(path: &[String], alias: String, message: String) -> Problem {
    let mut p = path.to_vec();
    p.push(alias);
    Problem { path: p, message }
}

// ---- extraction (capture aliases → JSON) ---------------------------------

/// The alias a node captures under, or `None` if it isn't captured (an unnamed
/// list/prose; a regex-titled heading without `@name`).
fn capture_alias(node: &Node) -> Option<String> {
    match node {
        Node::Heading { head, title, .. } => head.name.clone().or_else(|| match title {
            Match::Literal(t) => Some(slug(t)),
            Match::Regex(_) => None,
        }),
        Node::List { head, .. } | Node::Prose { head, .. } => head.name.clone(),
    }
}

/// A heading/range cardinality that yields an array rather than a single value.
fn repeated(card: Card) -> bool {
    matches!(card, Card::Plus | Card::Star | Card::Range(..))
}

fn extract_into(schema: &[Node], doc: &[DocBlock], obj: &mut Map<String, Value>) {
    for node in schema {
        let Some(key) = capture_alias(node) else {
            continue;
        };
        let value = match node {
            Node::Heading {
                level,
                title,
                head,
                children,
            } => {
                let secs: Vec<&DocBlock> = doc
                    .iter()
                    .filter(|b| is_section(b, *level, title))
                    .collect();
                let make = |sec: &DocBlock| {
                    let mut m = Map::new();
                    if let DocBlock::Section {
                        children: sc,
                        title: t,
                        ..
                    } = sec
                    {
                        if matches!(title, Match::Regex(_)) {
                            m.insert("title".into(), Value::String(t.clone()));
                        }
                        extract_into(children, sc, &mut m);
                    }
                    Value::Object(m)
                };
                if repeated(head.card) {
                    Value::Array(secs.iter().map(|s| make(s)).collect())
                } else {
                    secs.first().map(|s| make(s)).unwrap_or(Value::Null)
                }
            }
            Node::List {
                style,
                item,
                head,
                children,
            } => {
                let items: &[DocItem] = match doc.iter().find(|b| is_list(b, *style)) {
                    Some(DocBlock::List { items, .. }) => items,
                    _ => &[],
                };
                let single = matches!(head.card, Card::Required | Card::Optional);
                if single && children.is_empty() {
                    // A single labeled bullet captures the text after its label.
                    items
                        .first()
                        .map(|it| Value::String(after_label(item, &it.text)))
                        .unwrap_or(Value::Null)
                } else {
                    Value::Array(
                        items
                            .iter()
                            .map(|it| {
                                if children.is_empty() {
                                    Value::String(it.text.clone())
                                } else {
                                    let mut m = Map::new();
                                    m.insert("text".into(), Value::String(it.text.clone()));
                                    extract_into(children, &it.children, &mut m);
                                    Value::Object(m)
                                }
                            })
                            .collect(),
                    )
                }
            }
            Node::Prose { .. } => match doc.iter().find(|b| matches!(b, DocBlock::Para(_))) {
                Some(DocBlock::Para(t)) => Value::String(t.clone()),
                _ => Value::Null,
            },
        };
        obj.insert(key, value);
    }
}

/// The text after a literal label (`Docs: foo` with label `Docs:` → `foo`);
/// otherwise the whole text.
fn after_label(item: &Option<Match>, text: &str) -> String {
    if let Some(Match::Literal(l)) = item {
        if let Some(rest) = text.trim_start().strip_prefix(l.as_str()) {
            return rest.trim().to_string();
        }
    }
    text.to_string()
}
