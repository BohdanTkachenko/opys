//! Parsing of the markdown body: title, sections, and checklist items.

use std::sync::LazyLock;

use regex::Regex;

static TITLE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^# (.+)$").unwrap());
static CHECKED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^- \[x\] ").unwrap());
static UNCHECKED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^- \[ \] ").unwrap());

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
}
