//! Rendering: for now, `scaffold` — render a starter document from the schema
//! with placeholder values. Full data-driven `render` lands in a later phase.

use crate::schema::*;
use std::fmt::Write;

impl Schema {
    /// Emit a conforming starter document: required frontmatter keys with typed
    /// placeholders, headings, and one placeholder item per required list.
    /// Optional nodes (`?`/`*`/min-0) are omitted.
    pub fn scaffold(&self) -> String {
        let mut out = String::new();

        if !self.frontmatter.is_empty() {
            out.push_str("---\n");
            for f in &self.frontmatter {
                if f.optional {
                    continue;
                }
                let _ = writeln!(out, "{}: {}", f.key, fm_placeholder(&f.ty));
            }
            out.push_str("---\n\n");
        }

        scaffold_nodes(&self.body, &mut out, 0);
        // Trim trailing blank lines to a single newline.
        while out.ends_with("\n\n") {
            out.pop();
        }
        out
    }
}

fn fm_placeholder(ty: &FieldType) -> String {
    match ty {
        FieldType::Enum(vals) => vals.first().cloned().unwrap_or_default(),
        FieldType::List(_) => "[]".to_string(),
        _ => String::new(),
    }
}

/// A node is omitted from a scaffold when it isn't required to be present.
fn omitted(card: Card) -> bool {
    matches!(card, Card::Optional | Card::Star | Card::Range(0, _))
}

fn scaffold_nodes(nodes: &[Node], out: &mut String, indent: usize) {
    for node in nodes {
        match node {
            Node::Heading {
                level,
                title,
                head,
                children,
            } => {
                if omitted(head.card) {
                    continue;
                }
                let hashes = "#".repeat(*level as usize);
                let _ = writeln!(out, "{hashes} {}", literal_of(title)).map(|_| ());
                // headings reset to column 0; their content follows
                scaffold_nodes(children, out, 0);
                out.push('\n');
            }
            Node::List {
                style,
                item,
                head,
                children,
            } => {
                if omitted(head.card) {
                    continue;
                }
                let pad = " ".repeat(indent);
                let marker = match style {
                    ListStyle::Bullet => "- ",
                    ListStyle::Ordered => "1. ",
                    ListStyle::Checklist => "- [ ] ",
                };
                let lbl = item.as_ref().map(literal_of).unwrap_or_default();
                let _ = writeln!(out, "{pad}{marker}{lbl}");
                scaffold_nodes(children, out, indent + 2);
            }
            Node::Prose { head, text } => {
                if omitted(head.card) {
                    continue;
                }
                let lbl = text.as_ref().map(literal_of).unwrap_or_default();
                let _ = writeln!(out, "{lbl}");
            }
        }
    }
}

/// The literal text of a match for scaffolding; a regex contributes no text.
fn literal_of(m: &Match) -> String {
    match m {
        Match::Literal(s) => s.clone(),
        Match::Regex(_) => String::new(),
    }
}
