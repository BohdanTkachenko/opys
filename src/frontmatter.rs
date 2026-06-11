//! Frontmatter parsing and canonical serialization.
//!
//! Files are `---`-fenced YAML followed by a markdown body. Unlike the
//! original Python tool — which hand-parsed a "constrained" flat YAML — this
//! uses a real YAML parser (`serde_norway`), so custom fields may carry nested
//! mappings, sequences, and multiline/block scalars. The serializer still
//! emits canonical, minimal frontmatter (core fields first, then remaining
//! keys alphabetically), formatting flat scalars and scalar lists inline and
//! falling back to block YAML for complex custom values.

use serde_norway::{Mapping, Value};

/// Field keys with first-class meaning; everything else is a declared custom
/// field (or rejected by `verify`).
pub const RESERVED_FIELDS: [&str; 5] = ["id", "status", "tags", "spec", "wontfix_reason"];

const ORDER: [&str; 3] = ["id", "status", "tags"];

/// Parsed frontmatter, retaining the full YAML mapping so `verify` can inspect
/// wrong-typed values (rather than failing to parse them).
#[derive(Debug, Clone, Default)]
pub struct Frontmatter {
    pub map: Mapping,
}

impl Frontmatter {
    pub fn new() -> Self {
        Frontmatter {
            map: Mapping::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.map.get(Value::String(key.to_string()))
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.map.keys().filter_map(|k| k.as_str())
    }

    /// String value of `key`, only if it is actually a YAML string.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(|v| v.as_str())
    }

    pub fn id(&self) -> Option<&str> {
        self.get_str("id")
    }
    pub fn status(&self) -> Option<&str> {
        self.get_str("status")
    }
    pub fn wontfix_reason(&self) -> Option<&str> {
        self.get_str("wontfix_reason")
    }
    pub fn spec(&self) -> Option<&str> {
        self.get_str("spec")
    }

    /// `tags` as a list of strings, when it is a sequence whose elements are
    /// all strings. Returns `None` if absent or wrong-shaped (verify reports).
    pub fn tags(&self) -> Option<Vec<String>> {
        match self.get("tags")? {
            Value::Sequence(seq) => seq.iter().map(|v| v.as_str().map(str::to_string)).collect(),
            _ => None,
        }
    }

    /// True when `tags` is present and is a non-empty sequence.
    pub fn tags_is_nonempty_list(&self) -> bool {
        matches!(self.get("tags"), Some(Value::Sequence(s)) if !s.is_empty())
    }

    pub fn set_str(&mut self, key: &str, value: impl Into<String>) {
        self.map
            .insert(Value::String(key.to_string()), Value::String(value.into()));
    }

    pub fn set_tags(&mut self, tags: &[String]) {
        let seq = tags.iter().cloned().map(Value::String).collect();
        self.map
            .insert(Value::String("tags".to_string()), Value::Sequence(seq));
    }

    pub fn insert(&mut self, key: &str, value: Value) {
        self.map.insert(Value::String(key.to_string()), value);
    }
}

/// A frontmatter parse failure, carrying a human-readable message.
#[derive(Debug)]
pub struct ParseError(pub String);

/// Split a file into (frontmatter, body), then parse the frontmatter as YAML.
pub fn parse(text: &str, path_display: &str) -> Result<(Frontmatter, String), ParseError> {
    if !text.starts_with("---\n") {
        return Err(ParseError(format!(
            "{path_display}: missing frontmatter opening '---'"
        )));
    }
    let end = match text.get(4..).and_then(|s| s.find("\n---")) {
        Some(i) => i + 4,
        None => {
            return Err(ParseError(format!(
                "{path_display}: unterminated frontmatter"
            )))
        }
    };
    let yaml = &text[4..end];
    let mut body = &text[end + 4..];
    if let Some(stripped) = body.strip_prefix('\n') {
        body = stripped;
    }

    let map: Mapping = if yaml.trim().is_empty() {
        Mapping::new()
    } else {
        serde_norway::from_str(yaml).map_err(|e| ParseError(yaml_error(path_display, &e)))?
    };
    Ok((Frontmatter { map }, body.to_string()))
}

/// Build a frontmatter parse-error message, adding a targeted hint for the
/// most common footgun: an unquoted scalar containing a colon followed by a
/// space, which YAML reads as a nested mapping. (`opys`'s own serializer
/// quotes these, but files written by hand or by a script can trip it.)
fn yaml_error(path_display: &str, e: &serde_norway::Error) -> String {
    let mut msg = format!("{path_display}: invalid frontmatter YAML: {e}");
    if e.to_string().contains("mapping values are not allowed") {
        msg.push_str(
            " (hint: a value containing a colon followed by a space is read as nested \
             YAML — quote the whole value, e.g. key: \"text: more text\")",
        );
    }
    msg
}

/// Serialize frontmatter + body back into a complete file.
pub fn serialize(fm: &Frontmatter, body: &str) -> String {
    let mut keys: Vec<&str> = Vec::new();
    for k in ORDER {
        if fm.contains_key(k) {
            keys.push(k);
        }
    }
    let mut rest: Vec<&str> = fm.keys().filter(|k| !ORDER.contains(k)).collect();
    rest.sort_unstable();
    keys.extend(rest);

    let mut lines = String::from("---\n");
    for k in keys {
        let v = fm.get(k).expect("key came from the map");
        lines.push_str(&render_entry(k, v));
        lines.push('\n');
    }
    lines.push_str("---\n\n");
    lines.push_str(body.trim_start_matches('\n'));
    lines
}

/// Render one `key: value` frontmatter entry (possibly multi-line).
fn render_entry(key: &str, value: &Value) -> String {
    if let Some(scalar) = inline_scalar(value) {
        return format!("{key}: {scalar}");
    }
    if let Value::Sequence(seq) = value {
        if let Some(items) = seq.iter().map(inline_scalar).collect::<Option<Vec<_>>>() {
            return format!("{key}: [{}]", items.join(", "));
        }
    }
    // Complex value (nested mapping, or sequence with non-scalar elements):
    // emit it as block YAML under the key.
    let mut single = Mapping::new();
    single.insert(Value::String(key.to_string()), value.clone());
    serde_norway::to_string(&single)
        .unwrap_or_else(|_| format!("{key}: ~"))
        .trim_end_matches('\n')
        .to_string()
}

/// Inline representation of a flat scalar, or `None` if `value` is composite.
fn inline_scalar(value: &Value) -> Option<String> {
    match value {
        Value::Null => Some("null".to_string()),
        Value::Bool(b) => Some(if *b { "true" } else { "false" }.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(format_string(s)),
        _ => None,
    }
}

/// Quote a string only when needed for unambiguous YAML round-tripping.
fn format_string(s: &str) -> String {
    if needs_quote(s) {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

fn needs_quote(s: &str) -> bool {
    if s.is_empty() || s != s.trim() {
        return true;
    }
    // Would parse back as a non-string scalar.
    if matches!(s, "true" | "false" | "null" | "~" | "yes" | "no") {
        return true;
    }
    if s.parse::<i64>().is_ok() || s.parse::<f64>().is_ok() {
        return true;
    }
    // YAML indicator characters that make a plain scalar ambiguous.
    const SPECIAL: &str = ":#[]{}\"',&*!|>%@`";
    if s.chars().any(|c| SPECIAL.contains(c)) {
        return true;
    }
    matches!(s.chars().next(), Some('-') | Some('?'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_opening_fence() {
        assert!(parse("no fence here", "x.md").is_err());
    }

    #[test]
    fn unterminated() {
        assert!(parse("---\nid: A\n", "x.md").is_err());
    }

    #[test]
    fn unquoted_colon_space_value_gets_a_hint() {
        let text =
            "---\nid: A\nstatus: planned\ntags: [a]\nreason: MVP scope: containers\n---\n\n# T\n";
        let err = parse(text, "x.md").unwrap_err();
        assert!(err.0.contains("mapping values are not allowed"));
        assert!(err.0.contains("quote the whole value"));
    }

    #[test]
    fn parses_core_fields_and_body() {
        let text =
            "---\nid: VIK-0001\nstatus: planned\ntags: [osc, tabs]\n---\n\n# Title\n\nProse.\n";
        let (fm, body) = parse(text, "x.md").unwrap();
        assert_eq!(fm.id(), Some("VIK-0001"));
        assert_eq!(fm.status(), Some("planned"));
        assert_eq!(fm.tags(), Some(vec!["osc".into(), "tabs".into()]));
        // One leading newline is retained (matching the original tool); the
        // serializer trims it back out.
        assert_eq!(body, "\n# Title\n\nProse.\n");
        assert_eq!(crate::body::title(&body), "Title");
    }

    #[test]
    fn round_trip_key_order() {
        let mut fm = Frontmatter::new();
        fm.set_str("status", "planned");
        fm.set_str("id", "VIK-0001");
        fm.set_tags(&["osc".into(), "tabs".into()]);
        fm.set_str("ptyxis_ref", "src/x.c");
        let out = serialize(&fm, "# Title\n");
        let expected = "---\nid: VIK-0001\nstatus: planned\ntags: [osc, tabs]\nptyxis_ref: src/x.c\n---\n\n# Title\n";
        assert_eq!(out, expected);
    }

    #[test]
    fn quotes_ambiguous_strings() {
        let mut fm = Frontmatter::new();
        fm.set_str("id", "A-1");
        fm.set_str("ref", "set_title: handler");
        fm.set_str("numlike", "123");
        let out = serialize(&fm, "# T\n");
        assert!(out.contains("ref: \"set_title: handler\""));
        assert!(out.contains("numlike: \"123\""));
    }

    #[test]
    fn nested_custom_field_round_trips() {
        let text = "---\nid: VIK-0001\nstatus: planned\ntags: [a]\nlinks:\n- url: http://x\n  label: x\n---\n\n# T\n";
        let (fm, body) = parse(text, "x.md").unwrap();
        let out = serialize(&fm, &body);
        let (fm2, _) = parse(&out, "x.md").unwrap();
        assert_eq!(fm.get("links"), fm2.get("links"));
    }

    #[test]
    fn modernization_allows_block_scalar() {
        let text =
            "---\nid: VIK-0001\nstatus: planned\ntags: [a]\nnote: |\n  line one\n  line two\n---\n\n# T\n";
        let (fm, _) = parse(text, "x.md").unwrap();
        assert_eq!(fm.get_str("note"), Some("line one\nline two"));
    }
}
