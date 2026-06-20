//! Parser: DSL source → [`Schema`]. Line-oriented; indentation builds the body
//! tree. See `docs/structure-dsl-spec.md` for the grammar.

use crate::error::SchemaError;
use crate::schema::*;

/// Parse a schema from DSL source.
pub fn parse_schema(src: &str) -> Result<Schema, SchemaError> {
    let lines: Vec<&str> = src.lines().collect();
    let mut opts = SchemaOpts::default();
    let mut i = 0;

    // Phase 1: leading `%`-directives and `<? … ?>` / blank comment lines.
    while i < lines.len() {
        let (content, _) = split_desc(lines[i]);
        let t = content.trim();
        if t.is_empty() {
            i += 1;
        } else if let Some(rest) = t.strip_prefix('%') {
            apply_directive(rest, i + 1, &mut opts)?;
            i += 1;
        } else {
            break;
        }
    }

    // Phase 2: optional frontmatter block.
    let mut frontmatter = Vec::new();
    if i < lines.len() && split_desc(lines[i]).0.trim() == "---" {
        i += 1;
        loop {
            let line = lines.get(i).ok_or_else(|| {
                SchemaError::new(i, 1, "unterminated frontmatter (missing `---`)")
            })?;
            let (content, desc) = split_desc(line);
            if content.trim() == "---" {
                i += 1;
                break;
            }
            if !content.trim().is_empty() {
                let mut f = parse_field(content, i + 1)?;
                f.desc = desc;
                frontmatter.push(f);
            }
            i += 1;
        }
    }

    // Phase 3: body nodes, flat with indents, then assembled into a tree.
    let mut flat: Vec<(usize, Node)> = Vec::new();
    while i < lines.len() {
        let line = lines[i];
        let (content, desc) = split_desc(line);
        if content.trim().is_empty() {
            i += 1;
            continue;
        }
        let indent = content.len() - content.trim_start().len();
        let node = parse_node(content.trim_start(), desc, i + 1)?;
        flat.push((indent, node));
        i += 1;
    }
    let mut pos = 0;
    let body = build_tree(&flat, &mut pos, -1);

    Ok(Schema {
        opts,
        frontmatter,
        body,
    })
}

// ---- directives ----------------------------------------------------------

fn apply_directive(rest: &str, line: usize, opts: &mut SchemaOpts) -> Result<(), SchemaError> {
    let (k, v) = rest
        .split_once('=')
        .ok_or_else(|| SchemaError::new(line, 1, "directive needs `key = value`"))?;
    let (k, v) = (k.trim(), v.trim());
    match (k, v) {
        ("ordered", "true") => opts.ordered = true,
        ("ordered", "false") => opts.ordered = false,
        ("strict", "true") => opts.strict = true,
        ("strict", "false") => opts.strict = false,
        ("frontmatter", "open") => opts.frontmatter_open = true,
        ("frontmatter", "closed") => opts.frontmatter_open = false,
        _ => {
            return Err(SchemaError::new(
                line,
                1,
                format!("unknown directive `%{rest}`"),
            ))
        }
    }
    Ok(())
}

// ---- frontmatter ---------------------------------------------------------

fn parse_field(content: &str, line: usize) -> Result<FieldSchema, SchemaError> {
    let (left, ty_src) = content
        .split_once(':')
        .ok_or_else(|| SchemaError::new(line, 1, "frontmatter field needs `key: type`"))?;

    // left = key[?] [@alias]
    let left = left.trim();
    let key_end = left
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(left.len());
    if key_end == 0 {
        return Err(SchemaError::new(line, 1, "frontmatter field needs a key"));
    }
    let key = left[..key_end].to_string();
    let mut rest = left[key_end..].trim_start();
    let optional = if let Some(r) = rest.strip_prefix('?') {
        rest = r.trim_start();
        true
    } else {
        false
    };
    let alias = if let Some(r) = rest.strip_prefix('@') {
        r.trim().to_string()
    } else {
        key.clone()
    };

    let ty = parse_field_type(ty_src.trim(), line)?;
    Ok(FieldSchema {
        key,
        alias,
        optional,
        ty,
        desc: None,
    })
}

fn parse_field_type(s: &str, line: usize) -> Result<FieldType, SchemaError> {
    match s {
        "string" => return Ok(FieldType::Str),
        "int" => return Ok(FieldType::Int),
        "bool" => return Ok(FieldType::Bool),
        "date" => return Ok(FieldType::Date),
        _ => {}
    }
    if let Some(inner) = s.strip_prefix("enum(").and_then(|s| s.strip_suffix(')')) {
        let vals = inner
            .split(',')
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .collect();
        return Ok(FieldType::Enum(vals));
    }
    if let Some(inner) = s.strip_prefix('[') {
        // `[T]` with an optional trailing cardinality (e.g. `[string]+`).
        let close = inner
            .find(']')
            .ok_or_else(|| SchemaError::new(line, 1, "list type missing `]`"))?;
        let elem = parse_field_type(inner[..close].trim(), line)?;
        return Ok(FieldType::List(Box::new(elem)));
    }
    if s.starts_with('/') {
        return Ok(FieldType::Regex(parse_regex(s).0));
    }
    Err(SchemaError::new(line, 1, format!("unknown type `{s}`")))
}

// ---- body nodes ----------------------------------------------------------

fn parse_node(content: &str, desc: Option<String>, line: usize) -> Result<Node, SchemaError> {
    let (kind, head_src) = detect_marker(content)
        .ok_or_else(|| SchemaError::new(line, 1, format!("unrecognized marker: `{content}`")))?;

    let (card, rest) = parse_card(head_src);
    let (name, rest) = parse_name(rest);
    let label = parse_label(rest.trim());
    let head = Head { name, card, desc };

    Ok(match kind {
        MarkerKind::Heading(level) => Node::Heading {
            level,
            title: label.unwrap_or(Match::Regex(".*".into())),
            head,
            children: Vec::new(),
        },
        MarkerKind::Prose => Node::Prose { text: label, head },
        MarkerKind::Bullet => list(ListStyle::Bullet, label, head),
        MarkerKind::Ordered => list(ListStyle::Ordered, label, head),
        MarkerKind::Checklist => list(ListStyle::Checklist, label, head),
    })
}

fn list(style: ListStyle, item: Option<Match>, head: Head) -> Node {
    Node::List {
        style,
        item,
        head,
        children: Vec::new(),
    }
}

enum MarkerKind {
    Heading(u8),
    Bullet,
    Ordered,
    Checklist,
    Prose,
}

/// Detect the leading marker and return the kind + the remaining "head" text
/// (everything after the marker and its following spaces).
fn detect_marker(s: &str) -> Option<(MarkerKind, &str)> {
    if let Some(rest) = s.strip_prefix("- [ ]") {
        return Some((MarkerKind::Checklist, rest.trim_start()));
    }
    if s.starts_with('#') {
        let level = s.chars().take_while(|&c| c == '#').count();
        if (1..=6).contains(&level) {
            let after = &s[level..];
            // require a space (or end) after the hashes
            if after.is_empty() || after.starts_with(' ') {
                return Some((MarkerKind::Heading(level as u8), after.trim_start()));
            }
        }
        return None;
    }
    if let Some(rest) = s.strip_prefix("- ") {
        return Some((MarkerKind::Bullet, rest.trim_start()));
    }
    if s == "-" {
        return Some((MarkerKind::Bullet, ""));
    }
    // ordered: digits then '.'
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && s[digits..].starts_with('.') {
        return Some((MarkerKind::Ordered, s[digits + 1..].trim_start()));
    }
    if let Some(rest) = s.strip_prefix('>') {
        return Some((MarkerKind::Prose, rest.trim_start()));
    }
    None
}

fn parse_card(s: &str) -> (Card, &str) {
    match s.as_bytes().first() {
        Some(b'+') => (Card::Plus, &s[1..]),
        Some(b'*') => (Card::Star, &s[1..]),
        Some(b'?') => (Card::Optional, &s[1..]),
        Some(b'{') => parse_range(s),
        _ => (Card::Required, s),
    }
}

fn parse_range(s: &str) -> (Card, &str) {
    let Some(close) = s.find('}') else {
        return (Card::Required, s);
    };
    let body = &s[1..close];
    let (min_s, max_s) = match body.split_once(',') {
        Some((a, b)) => (a.trim(), Some(b.trim())),
        None => (body.trim(), None),
    };
    let Ok(min) = min_s.parse::<u32>() else {
        return (Card::Required, s);
    };
    let max = match max_s {
        None => Some(min),
        Some("") => None,
        Some(m) => match m.parse::<u32>() {
            Ok(v) => Some(v),
            Err(_) => return (Card::Required, s),
        },
    };
    (Card::Range(min, max), &s[close + 1..])
}

fn parse_name(s: &str) -> (Option<String>, &str) {
    let Some(rest) = s.strip_prefix('@') else {
        return (None, s);
    };
    let end = rest
        .find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
        .unwrap_or(rest.len());
    if end == 0 {
        return (None, s);
    }
    (Some(rest[..end].to_string()), &rest[end..])
}

fn parse_label(s: &str) -> Option<Match> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if s.starts_with('/') {
        return Some(Match::Regex(parse_regex(s).0));
    }
    if let Some(rest) = s.strip_prefix('"') {
        let lit = match rest.rsplit_once('"') {
            Some((inner, _)) => inner,
            None => rest,
        };
        return Some(Match::Literal(unescape(lit)));
    }
    Some(Match::Literal(unescape(s)))
}

/// Parse `/pattern/flags` → a regex pattern string (flags folded into `(?…)`).
/// Returns `(pattern, rest_after)`.
fn parse_regex(s: &str) -> (String, &str) {
    debug_assert!(s.starts_with('/'));
    let body = &s[1..];
    // find the closing unescaped '/'
    let mut end = None;
    let bytes = body.as_bytes();
    let mut k = 0;
    while k < bytes.len() {
        if bytes[k] == b'\\' {
            k += 2;
            continue;
        }
        if bytes[k] == b'/' {
            end = Some(k);
            break;
        }
        k += 1;
    }
    let Some(end) = end else {
        return (body.to_string(), "");
    };
    let pat = &body[..end];
    let after = &body[end + 1..];
    let flag_len = after
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .count();
    let flags = &after[..flag_len];
    let pattern = if flags.is_empty() {
        pat.to_string()
    } else {
        format!("(?{flags}){pat}")
    };
    (pattern, &after[flag_len..])
}

/// Resolve backslash escapes: `\x` → `x`.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

// ---- description / comment splitting -------------------------------------

/// Split a line into its content and an optional `<? … ?>` description. The
/// first unescaped `<?` starts the description; it runs to the trailing `?>`.
fn split_desc(line: &str) -> (&str, Option<String>) {
    let bytes = line.as_bytes();
    let mut k = 0;
    while k + 1 < bytes.len() {
        if bytes[k] == b'\\' {
            k += 2;
            continue;
        }
        if bytes[k] == b'<' && bytes[k + 1] == b'?' {
            let before = &line[..k];
            let rest = &line[k + 2..];
            let inner = rest.strip_suffix("?>").unwrap_or(rest);
            return (before, Some(inner.trim().to_string()));
        }
        k += 1;
    }
    (line, None)
}

// ---- tree assembly -------------------------------------------------------

/// Build the node tree from flat `(indent, node)` pairs: a node is a child of
/// the most recent node with strictly smaller indent.
fn build_tree(flat: &[(usize, Node)], pos: &mut usize, parent_indent: isize) -> Vec<Node> {
    let mut out = Vec::new();
    while *pos < flat.len() {
        let indent = flat[*pos].0 as isize;
        if indent <= parent_indent {
            break;
        }
        let mut node = flat[*pos].1.clone();
        *pos += 1;
        let children = build_tree(flat, pos, indent);
        node.set_children(children);
        out.push(node);
    }
    out
}
