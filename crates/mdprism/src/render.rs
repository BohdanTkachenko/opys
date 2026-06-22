//! Rendering: `scaffold` (schema → starter document) and `render` (data → document).

use crate::error::RenderError;
use crate::schema::*;
use serde_json::Value;
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

    /// Render a conforming markdown document from `data` — the inverse of
    /// [`Schema::extract`]. `data` is a JSON object keyed by the capture aliases
    /// declared in the schema.
    ///
    /// Returns [`RenderError::MissingField`] if a required field is absent from
    /// `data`, or [`RenderError::WrongType`] if a value has an incompatible JSON
    /// type.
    pub fn render(&self, data: &Value) -> Result<String, RenderError> {
        let mut out = String::new();

        if !self.frontmatter.is_empty() {
            out.push_str("---\n");
            if let Value::Object(obj) = data {
                for f in &self.frontmatter {
                    match obj.get(&f.alias) {
                        Some(v) if !v.is_null() => {
                            let _ = writeln!(out, "{}: {}", f.key, render_fm_value(v));
                        }
                        None if !f.optional => {
                            return Err(RenderError::MissingField(f.alias.clone()));
                        }
                        _ => {} // null or optional-missing → skip
                    }
                }
            } else {
                // data is not an object — check if any required frontmatter fields exist
                if let Some(f) = self.frontmatter.iter().find(|f| !f.optional) {
                    return Err(RenderError::MissingField(f.alias.clone()));
                }
            }
            out.push_str("---\n\n");
        }

        render_nodes(&self.body, data, &mut out, 0)?;

        // Trim trailing blank lines to a single newline.
        while out.ends_with("\n\n") {
            out.pop();
        }

        Ok(out)
    }
}

// ---- render helpers ---------------------------------------------------------

fn render_nodes(
    nodes: &[Node],
    scope: &Value,
    out: &mut String,
    list_indent: usize,
) -> Result<(), RenderError> {
    for node in nodes {
        let Some(key) = node_alias(node) else {
            continue;
        };
        let value = scope.get(&key).unwrap_or(&Value::Null);
        render_node(node, &key, value, out, list_indent)?;
    }
    Ok(())
}

fn render_node(
    node: &Node,
    key: &str,
    value: &Value,
    out: &mut String,
    list_indent: usize,
) -> Result<(), RenderError> {
    match node {
        Node::Heading {
            level,
            title,
            head,
            children,
        } => {
            if value.is_null() {
                if !omitted(head.card) {
                    return Err(RenderError::MissingField(key.to_string()));
                }
                return Ok(());
            }
            let hashes = "#".repeat(*level as usize);
            if is_repeated(head.card) {
                let arr = value.as_array().ok_or_else(|| RenderError::WrongType {
                    field: key.to_string(),
                    expected: "array",
                })?;
                for item in arr {
                    let t = heading_title(title, item);
                    let _ = writeln!(out, "{hashes} {t}");
                    render_nodes(children, item, out, 0)?;
                    out.push('\n');
                }
            } else {
                let t = heading_title(title, value);
                let _ = writeln!(out, "{hashes} {t}");
                render_nodes(children, value, out, 0)?;
                out.push('\n');
            }
        }

        Node::List {
            style,
            item: label,
            head,
            children,
        } => {
            if value.is_null() {
                if !omitted(head.card) {
                    return Err(RenderError::MissingField(key.to_string()));
                }
                return Ok(());
            }
            let pad = " ".repeat(list_indent);
            let marker = match style {
                ListStyle::Bullet => "- ",
                ListStyle::Ordered => "1. ",
                ListStyle::Checklist => "- [ ] ",
            };
            let label_prefix = match label {
                Some(Match::Literal(l)) => format!("{l} "),
                _ => String::new(),
            };

            let single = matches!(head.card, Card::Required | Card::Optional);
            if single && children.is_empty() {
                // single labeled item → value is a String
                let text = value.as_str().unwrap_or("");
                let _ = writeln!(out, "{pad}{marker}{label_prefix}{text}");
            } else {
                match value {
                    Value::Array(arr) => {
                        for item_val in arr {
                            if children.is_empty() {
                                let text = item_val.as_str().unwrap_or("");
                                let _ = writeln!(out, "{pad}{marker}{label_prefix}{text}");
                            } else {
                                let text = item_val
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .or_else(|| item_val.as_str())
                                    .unwrap_or("");
                                let _ = writeln!(out, "{pad}{marker}{label_prefix}{text}");
                                render_nodes(children, item_val, out, list_indent + 2)?;
                            }
                        }
                    }
                    Value::String(s) => {
                        // single value where array was expected — render as one item
                        let _ = writeln!(out, "{pad}{marker}{label_prefix}{s}");
                    }
                    _ => {
                        return Err(RenderError::WrongType {
                            field: key.to_string(),
                            expected: "array or string",
                        });
                    }
                }
            }
        }

        Node::Prose { head, .. } => {
            if value.is_null() {
                if !omitted(head.card) {
                    return Err(RenderError::MissingField(key.to_string()));
                }
                return Ok(());
            }
            let text = value.as_str().unwrap_or("");
            let _ = writeln!(out, "{text}");
            out.push('\n');
        }
    }
    Ok(())
}

/// The literal title to use for a heading: use the schema literal, or fall
/// back to `value["title"]` for regex headings.
fn heading_title(title: &Match, value: &Value) -> String {
    match title {
        Match::Literal(s) => s.clone(),
        Match::Regex(_) => value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }
}

/// The alias a node captures under for rendering (same logic as extraction).
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

/// A node is omitted from scaffold/render when it isn't required to be present.
fn omitted(card: Card) -> bool {
    matches!(card, Card::Optional | Card::Star | Card::Range(0, _))
}

/// Cardinality that produces multiple values (array in extracted data).
fn is_repeated(card: Card) -> bool {
    matches!(card, Card::Plus | Card::Star | Card::Range(..))
}

// ---- scaffold helpers -------------------------------------------------------

fn fm_placeholder(ty: &FieldType) -> String {
    match ty {
        FieldType::Enum(vals) => vals.first().cloned().unwrap_or_default(),
        FieldType::List(_) => "[]".to_string(),
        _ => String::new(),
    }
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

fn render_fm_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(render_fm_value).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Null => String::new(),
        Value::Object(_) => v.to_string(),
    }
}

/// The literal text of a match for scaffolding; a regex contributes no text.
fn literal_of(m: &Match) -> String {
    match m {
        Match::Literal(s) => s.clone(),
        Match::Regex(_) => String::new(),
    }
}
