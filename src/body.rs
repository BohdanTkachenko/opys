//! Parsing of the markdown body: title, sections, checklist items, and
//! `structured` section items.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

static TITLE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^# (.+)$").unwrap());
static CHECKED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^- \[x\] ").unwrap());
static UNCHECKED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^- \[ \] ").unwrap());
static STEP_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+\.").unwrap());

/// First `# Heading` line, or `""`.
pub fn title(body: &str) -> String {
    TITLE_RE
        .captures(body)
        .map(|c| c[1].trim().to_string())
        .unwrap_or_default()
}

/// Text of the `## <header>` section, up to the next `## ` heading (or EOF).
pub fn section(body: &str, header: &str) -> String {
    let re = Regex::new(&format!(r"(?m)^## {}\s*$", regex::escape(header))).unwrap();
    let Some(m) = re.find(body) else {
        return String::new();
    };
    let rest = &body[m.end()..];
    let next = Regex::new(r"(?m)^## ").unwrap();
    match next.find(rest) {
        Some(n) => rest[..n.start()].to_string(),
        None => rest.to_string(),
    }
}

#[derive(Debug, Clone)]
pub struct ChecklistItem {
    pub checked: bool,
    pub line: String,
}

/// Top-level checkbox items under the named `## <header>` section. Used for a
/// feature's `## Test plan` and a work item's `## Tasks`.
pub fn checklist_items(body: &str, header: &str) -> Vec<ChecklistItem> {
    let mut out = Vec::new();
    for line in section(body, header).lines() {
        if CHECKED_RE.is_match(line) {
            out.push(ChecklistItem {
                checked: true,
                line: line.to_string(),
            });
        } else if UNCHECKED_RE.is_match(line) {
            out.push(ChecklistItem {
                checked: false,
                line: line.to_string(),
            });
        }
    }
    out
}

/// Whether the body contains a `## <header>` section heading.
pub fn has_section(body: &str, header: &str) -> bool {
    Regex::new(&format!(r"(?m)^## {}\s*$", regex::escape(header)))
        .unwrap()
        .is_match(body)
}

/// One item of a `structured` section: its lead `- <desc>` line plus the named
/// parts found under it. `values` holds `- <Label>: <value>` bullets; `ordered`
/// holds `- <Label>:` bullets whose numbered list follows. The set of part
/// labels comes from config — this parser is label-agnostic.
#[derive(Debug, Clone)]
pub struct StructuredItem {
    pub desc: String,
    pub values: BTreeMap<String, String>,
    pub ordered: BTreeMap<String, Vec<String>>,
}

impl StructuredItem {
    /// Whether the named part is present: a non-empty `value` part, or an
    /// `ordered` part with at least one numbered entry.
    pub fn has_part(&self, label: &str) -> bool {
        self.values.contains_key(label) || self.ordered.get(label).is_some_and(|v| !v.is_empty())
    }
}

/// Items under a `structured` `## <heading>`. A column-0 `- ` line starts an
/// item; an indented `- <Label>: <value>` bullet is a value part, and an
/// indented `- <Label>:` bullet followed by a numbered list is an ordered part.
pub fn structured_items(body: &str, heading: &str) -> Vec<StructuredItem> {
    let mut items: Vec<StructuredItem> = Vec::new();
    // The ordered part currently collecting numbered lines (None after a value
    // part or a new item).
    let mut current: Option<String> = None;
    for line in section(body, heading).lines() {
        if let Some(rest) = line.strip_prefix("- ") {
            items.push(StructuredItem {
                desc: rest.trim().to_string(),
                values: BTreeMap::new(),
                ordered: BTreeMap::new(),
            });
            current = None;
            continue;
        }
        let Some(cur) = items.last_mut() else {
            continue;
        };
        let s = line.trim();
        if let Some(bullet) = s.strip_prefix("- ") {
            match bullet.split_once(':') {
                // `- Label:` (empty value) opens an ordered part.
                Some((label, value)) if value.trim().is_empty() => {
                    let label = label.trim().to_string();
                    cur.ordered.entry(label.clone()).or_default();
                    current = Some(label);
                }
                // `- Label: value` is a value part.
                Some((label, value)) => {
                    cur.values
                        .insert(label.trim().to_string(), value.trim().to_string());
                    current = None;
                }
                None => current = None,
            }
        } else if STEP_RE.is_match(s) {
            if let Some(label) = &current {
                let step = STEP_RE.replace(s, "").trim_start().to_string();
                cur.ordered.entry(label.clone()).or_default().push(step);
            }
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "# Tab title\n\n## Test plan\n- [x] valid UTF-8 — `tab::osc_title`\n- [ ] invalid UTF-8 — uncovered\n\n## Manual verification\n- Legible at scaling — *manual: rendering*\n  - Setup: external monitor at 150%\n  - Steps:\n    1. Open a tab\n    2. printf escape\n  - Expect: crisp glyphs\n";

    #[test]
    fn finds_title() {
        assert_eq!(title(BODY), "Tab title");
    }

    #[test]
    fn parses_test_plan() {
        let items = checklist_items(BODY, "Test plan");
        assert_eq!(items.len(), 2);
        assert!(items[0].checked);
        assert!(!items[1].checked);
        assert!(items[0].line.contains("tab::osc_title"));
    }

    #[test]
    fn parses_structured_item() {
        let items = structured_items(BODY, "Manual verification");
        assert_eq!(items.len(), 1);
        let it = &items[0];
        assert_eq!(
            it.values.get("Setup").map(String::as_str),
            Some("external monitor at 150%")
        );
        assert_eq!(it.ordered.get("Steps").map(Vec::len), Some(2));
        assert_eq!(it.ordered["Steps"][0], "Open a tab");
        assert_eq!(
            it.values.get("Expect").map(String::as_str),
            Some("crisp glyphs")
        );
        // Convenience: required-part presence.
        assert!(it.has_part("Setup"));
        assert!(it.has_part("Steps"));
        assert!(!it.has_part("Teardown"));
    }
}
