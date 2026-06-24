//! In-place editing: replace the text content of a named node identified by
//! a dot-separated alias path, leaving the rest of the document byte-for-byte
//! identical.

use super::error::EditError;
use super::schema::*;
use comrak::nodes::{AstNode, ListType, NodeValue};
use comrak::{parse_document, Arena, Options};

impl Schema {
    /// Replace the content of the node at `target` (a `.`-separated alias
    /// path, e.g. `"plan.cases.2"`) with `new_value` and return the modified
    /// markdown.
    ///
    /// `target` mirrors the key path of [`Schema::extract`]'s output; a
    /// trailing numeric segment (`.2`) picks the N-th element of a list.
    /// Only leaf string nodes (list items and prose paragraphs) are editable;
    /// for list items the bullet/marker and any checkbox prefix are preserved.
    ///
    /// Returns [`EditError::TargetNotFound`] if the path does not resolve, or
    /// [`EditError::IndexOutOfRange`] if a numeric index exceeds the list.
    pub fn edit(&self, md: &str, target: &str, new_value: &str) -> Result<String, EditError> {
        let body = strip_frontmatter(md);
        let body_offset = md.len() - body.len();

        let ls = line_start_offsets(body);
        let arena = Arena::new();
        let root = parse_document(&arena, body, &Options::default());
        let blocks = build_sp_blocks(root.children(), &ls);

        let path: Vec<&str> = target.split('.').collect();
        let span = find_span(&path, &self.body, &blocks).ok_or(EditError::TargetNotFound)?;

        let abs_start = body_offset + span.start;
        let abs_end = body_offset + span.end;

        let mut out = String::with_capacity(md.len());
        out.push_str(&md[..abs_start]);
        out.push_str(new_value);
        out.push_str(&md[abs_end..]);
        Ok(out)
    }
}

// ---- source-position-aware block tree ----------------------------------------

/// Byte range within the body string (`start` inclusive, `end` exclusive).
#[derive(Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
}

struct SpBlock {
    kind: SpBlockKind,
}

#[allow(dead_code)]
enum SpBlockKind {
    Section {
        level: u8,
        title: String,
        children: Vec<SpBlock>,
    },
    List {
        ordered: bool,
        items: Vec<SpItem>,
    },
    Para {
        span: Span,
    },
}

#[allow(dead_code)]
struct SpItem {
    text: String,
    /// Byte span of the editable text (after the list marker and any checkbox).
    text_span: Span,
    checked: Option<bool>,
    children: Vec<SpBlock>,
}

// Two-stage build: first flatten into a mixed heading/block sequence, then nest.

enum FlatItem {
    Heading { level: u8, title: String },
    Block { idx: usize },
}

fn build_sp_blocks<'a>(nodes: impl Iterator<Item = &'a AstNode<'a>>, ls: &[usize]) -> Vec<SpBlock> {
    let mut flat: Vec<FlatItem> = Vec::new();
    let mut store: Vec<Option<SpBlock>> = Vec::new();

    for node in nodes {
        let val = node.data.borrow().value.clone();
        match val {
            NodeValue::Heading(h) => {
                flat.push(FlatItem::Heading {
                    level: h.level,
                    title: text_of(node),
                });
            }
            NodeValue::List(nl) => {
                let ordered = matches!(nl.list_type, ListType::Ordered);
                let items = build_sp_items(node, ls);
                let idx = store.len();
                store.push(Some(SpBlock {
                    kind: SpBlockKind::List { ordered, items },
                }));
                flat.push(FlatItem::Block { idx });
            }
            NodeValue::Paragraph => {
                if let Some(sp) = first_text_span(node, ls) {
                    let idx = store.len();
                    store.push(Some(SpBlock {
                        kind: SpBlockKind::Para { span: sp },
                    }));
                    flat.push(FlatItem::Block { idx });
                }
            }
            _ => {}
        }
    }

    let mut pos = 0;
    sp_nest(&flat, &mut store, &mut pos, 0)
}

fn sp_nest(
    flat: &[FlatItem],
    store: &mut Vec<Option<SpBlock>>,
    pos: &mut usize,
    parent_level: usize,
) -> Vec<SpBlock> {
    let mut out: Vec<SpBlock> = Vec::new();
    while *pos < flat.len() {
        match &flat[*pos] {
            FlatItem::Heading { level, title } => {
                if (*level as usize) <= parent_level {
                    break;
                }
                let (level, title) = (*level, title.clone());
                *pos += 1;
                let children = sp_nest(flat, store, pos, level as usize);
                out.push(SpBlock {
                    kind: SpBlockKind::Section {
                        level,
                        title,
                        children,
                    },
                });
            }
            FlatItem::Block { idx } => {
                let idx = *idx;
                *pos += 1;
                if let Some(block) = store.get_mut(idx).and_then(|b| b.take()) {
                    out.push(block);
                }
            }
        }
    }
    out
}

fn build_sp_items<'a>(list_node: &'a AstNode<'a>, ls: &[usize]) -> Vec<SpItem> {
    let mut items = Vec::new();
    for item in list_node.children() {
        let mut raw_text = String::new();
        let mut raw_span = Span { start: 0, end: 0 };
        let mut child_list_nodes: Vec<&AstNode<'a>> = Vec::new();

        for ch in item.children() {
            let val = ch.data.borrow().value.clone();
            match val {
                NodeValue::Paragraph if raw_text.is_empty() => {
                    raw_text = text_of(ch);
                    if let Some(sp) = first_text_span(ch, ls) {
                        raw_span = sp;
                    }
                }
                NodeValue::List(_) => child_list_nodes.push(ch),
                _ => {}
            }
        }

        let (checked, content, checkbox_bytes) = split_checkbox(&raw_text);
        raw_span.start += checkbox_bytes;

        let children = child_list_nodes
            .into_iter()
            .map(|n| {
                let val = n.data.borrow().value.clone();
                let ordered = matches!(
                    val,
                    NodeValue::List(ref nl) if matches!(nl.list_type, ListType::Ordered)
                );
                SpBlock {
                    kind: SpBlockKind::List {
                        ordered,
                        items: build_sp_items(n, ls),
                    },
                }
            })
            .collect();

        items.push(SpItem {
            text: content,
            text_span: raw_span,
            checked,
            children,
        });
    }
    items
}

/// Byte span of the first `Text` child's content, relative to the body string.
fn first_text_span<'a>(node: &'a AstNode<'a>, ls: &[usize]) -> Option<Span> {
    for ch in node.children() {
        let data = ch.data.borrow();
        if let NodeValue::Text(ref t) = data.value {
            let sp = data.sourcepos;
            let start = linecol_to_offset(sp.start.line, sp.start.column, ls);
            let end = start + t.len();
            return Some(Span { start, end });
        }
    }
    None
}

fn linecol_to_offset(line: usize, col: usize, ls: &[usize]) -> usize {
    ls.get(line.saturating_sub(1)).copied().unwrap_or(0) + col.saturating_sub(1)
}

fn line_start_offsets(s: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, &b) in s.as_bytes().iter().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

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

fn split_checkbox(text: &str) -> (Option<bool>, String, usize) {
    if let Some(rest) = text.strip_prefix("[ ] ") {
        (Some(false), rest.to_string(), 4)
    } else if let Some(rest) = text
        .strip_prefix("[x] ")
        .or_else(|| text.strip_prefix("[X] "))
    {
        (Some(true), rest.to_string(), 4)
    } else {
        (None, text.to_string(), 0)
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

// ---- path navigation ---------------------------------------------------------

/// Walk the schema + SpBlock tree following `path` and return the byte span of
/// the target leaf's editable text.
fn find_span(path: &[&str], schema: &[Node], blocks: &[SpBlock]) -> Option<Span> {
    if path.is_empty() {
        return None;
    }
    let key = path[0];
    let rest = &path[1..];

    // Walk schema nodes in order; for each, advance into the corresponding doc block.
    let mut block_idx = 0usize;
    for snode in schema {
        let alias = match node_alias(snode) {
            Some(a) => a,
            None => {
                block_idx += 1;
                continue;
            }
        };

        // Skip doc blocks that don't match the current schema node type.
        let block = match advance_to_matching(snode, blocks, &mut block_idx) {
            Some(b) => b,
            None => continue,
        };

        if alias != key {
            block_idx += 1;
            continue;
        }

        return match (snode, block) {
            (
                Node::Heading {
                    children: schema_ch,
                    ..
                },
                SpBlock {
                    kind:
                        SpBlockKind::Section {
                            children: doc_ch, ..
                        },
                },
            ) => {
                if rest.is_empty() {
                    None // Can't edit a whole section heading (not yet supported)
                } else {
                    find_span(rest, schema_ch, doc_ch)
                }
            }

            (
                Node::List { head, .. },
                SpBlock {
                    kind: SpBlockKind::List { items, .. },
                },
            ) => {
                if rest.is_empty() {
                    return None;
                }
                let idx: usize = rest[0].parse().ok()?;
                let remaining = &rest[1..];
                let item = items.get(idx)?;
                if remaining.is_empty() {
                    Some(item.text_span)
                } else {
                    // Nested list navigation: recurse with item's children.
                    // Build a synthetic schema for item children — use the
                    // schema list's children as the per-item schema.
                    let _ = head;
                    let item_schema: &[Node] = match snode {
                        Node::List { children, .. } => children,
                        _ => &[],
                    };
                    find_span(remaining, item_schema, &item.children)
                }
            }

            (
                Node::Prose { .. },
                SpBlock {
                    kind: SpBlockKind::Para { span },
                },
            ) => {
                if rest.is_empty() {
                    Some(*span)
                } else {
                    None
                }
            }

            _ => None,
        };
    }
    None
}

/// Find the doc block at or after `block_idx` that structurally matches the
/// given schema node (heading↔section, list↔list, prose↔para), advance
/// `block_idx` to point at it, and return a reference. Returns `None` if no
/// matching block is found within the remaining blocks.
fn advance_to_matching<'a>(
    snode: &Node,
    blocks: &'a [SpBlock],
    block_idx: &mut usize,
) -> Option<&'a SpBlock> {
    while *block_idx < blocks.len() {
        let b = &blocks[*block_idx];
        let is_match = matches!(
            (snode, &b.kind),
            (Node::Heading { .. }, SpBlockKind::Section { .. })
                | (Node::List { .. }, SpBlockKind::List { .. })
                | (Node::Prose { .. }, SpBlockKind::Para { .. })
        );
        if is_match {
            return Some(b);
        }
        *block_idx += 1;
    }
    None
}

fn node_alias(node: &Node) -> Option<String> {
    match node {
        Node::Heading { head, title, .. } => head.name.clone().or_else(|| match title {
            Match::Literal(t) => Some(slug(t)),
            Match::Regex(_) => None,
        }),
        Node::List { head, .. } | Node::Prose { head, .. } => head.name.clone(),
    }
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
