//! Parsing of the markdown body: title, `## Test plan`, `## Manual verification`.

use std::sync::LazyLock;

use regex::Regex;

static TITLE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^# (.+)$").unwrap());
static CHECKED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^- \[x\] ").unwrap());
static UNCHECKED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^- \[ \] ").unwrap());
static TEST_REF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([^`]+)`").unwrap());
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

/// Top-level checkbox items under `## Test plan`.
pub fn test_plan_items(body: &str) -> Vec<ChecklistItem> {
    checklist_items(body, "Test plan")
}

/// Whether the body contains a `## <header>` section heading.
pub fn has_section(body: &str, header: &str) -> bool {
    Regex::new(&format!(r"(?m)^## {}\s*$", regex::escape(header)))
        .unwrap()
        .is_match(body)
}

/// Backticked test references on a test-plan or manual-verification line.
///
/// Only spans shaped like a reference — containing the `::` module/path
/// separator (`mod::test_name`, `path/to/file.rs::test_name`) — are returned.
/// Inline code spans used in the item's *prose* (shell snippets, literals,
/// escape sequences) are deliberately ignored, so a checked item can explain
/// itself with backticked code without that code being mistaken for a ref.
pub fn test_refs(line: &str) -> Vec<String> {
    TEST_REF_RE
        .captures_iter(line)
        .map(|c| c[1].to_string())
        .filter(|s| is_test_ref(s))
        .collect()
}

/// Whether a backtick span has the shape of a test reference. A reference is
/// always `module::name` or `path/to/file::name`, so the `::` separator is
/// what distinguishes a real reference from a prose code span.
pub fn is_test_ref(span: &str) -> bool {
    span.contains("::")
}

#[derive(Debug, Clone)]
pub struct ManualItem {
    pub desc: String,
    pub setup: Option<String>,
    pub steps: Vec<String>,
    pub expect: Option<String>,
    /// Backticked test references on the item's description line. An item with
    /// no refs has no automated coverage and is prioritized for manual runs.
    pub refs: Vec<String>,
}

impl ManualItem {
    /// True when no automated test backs this manual check.
    pub fn uncovered(&self) -> bool {
        self.refs.is_empty()
    }
}

/// Structured items under `## Manual verification`. A column-0 `- ` line
/// starts a new item; indented bullets supply its Setup/Steps/Expect.
pub fn manual_items(body: &str) -> Vec<ManualItem> {
    let mut items: Vec<ManualItem> = Vec::new();
    for line in section(body, "Manual verification").lines() {
        if let Some(rest) = line.strip_prefix("- ") {
            items.push(ManualItem {
                desc: rest.trim().to_string(),
                setup: None,
                steps: Vec::new(),
                expect: None,
                refs: test_refs(rest),
            });
            continue;
        }
        let Some(cur) = items.last_mut() else {
            continue;
        };
        let s = line.trim();
        if let Some(v) = s.strip_prefix("- Setup:") {
            cur.setup = Some(v.trim().to_string());
        } else if let Some(v) = s.strip_prefix("- Expect:") {
            cur.expect = Some(v.trim().to_string());
        } else if STEP_RE.is_match(s) {
            cur.steps
                .push(STEP_RE.replace(s, "").trim_start().to_string());
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
        let items = test_plan_items(BODY);
        assert_eq!(items.len(), 2);
        assert!(items[0].checked);
        assert!(!items[1].checked);
        assert_eq!(
            test_refs(&items[0].line),
            vec!["tab::osc_title".to_string()]
        );
    }

    #[test]
    fn ignores_prose_code_spans() {
        // A real ref plus a prose code span on the same checked item: only the
        // `::`-shaped span is a reference; the shell snippet is left as prose.
        let line = "- [x] sftp:// rewrites to `ssh -t exec $SHELL -l` not a path — \
                    `application.rs::sftp_uri_rewrites_to_ssh`";
        assert_eq!(
            test_refs(line),
            vec!["application.rs::sftp_uri_rewrites_to_ssh".to_string()]
        );
    }

    #[test]
    fn prose_only_span_is_not_a_ref() {
        // No `::` anywhere: nothing is treated as a reference (so verify will
        // correctly flag a checked item that lacks a real ref).
        let line = "- [x] split_command handles quotes (`bash -c \"echo hi\"` is 3 argv)";
        assert!(test_refs(line).is_empty());
    }

    #[test]
    fn parses_manual_item() {
        let items = manual_items(BODY);
        assert_eq!(items.len(), 1);
        let it = &items[0];
        assert_eq!(it.setup.as_deref(), Some("external monitor at 150%"));
        assert_eq!(it.steps.len(), 2);
        assert_eq!(it.steps[0], "Open a tab");
        assert_eq!(it.expect.as_deref(), Some("crisp glyphs"));
    }
}
