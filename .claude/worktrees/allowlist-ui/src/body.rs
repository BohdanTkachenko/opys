//! Parsing of the markdown body: title, sections, and checklist items.

use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;

static TITLE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^# (.+)$").unwrap());
static CHECKED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^- \[x\] ").unwrap());
static UNCHECKED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^- \[ \] ").unwrap());
static SECTION_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^## +(.*?)\s*$").unwrap());

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

/// Split the body into `## ` sections in document order as `(heading, content)`
/// pairs — `content` is the text under each heading up to the next `## `. The
/// first pair has an empty heading and holds the preamble (title + intro) before
/// the first `## `. Follows [`section`]'s `## `-delimited convention.
pub fn sections(body: &str) -> Vec<(String, String)> {
    section_spans(body)
        .into_iter()
        .map(|(h, r)| (h, body[r].trim().to_string()))
        .collect()
}

/// The byte range of each `## ` section's *content* (the text under the heading,
/// up to the next `## `), paired with its heading, in document order — the
/// preamble (empty heading) first unless it is blank. The `blocks` projection's
/// `seq` indexes into this, so it is the addressing scheme for section edits.
pub fn section_spans(body: &str) -> Vec<(String, Range<usize>)> {
    let mut raw: Vec<(String, Range<usize>)> = Vec::new();
    let mut content_start = 0;
    let mut heading = String::new();
    for m in SECTION_RE.captures_iter(body) {
        let whole = m.get(0).unwrap();
        raw.push((std::mem::take(&mut heading), content_start..whole.start()));
        heading = m[1].trim().to_string();
        content_start = whole.end();
    }
    raw.push((heading, content_start..body.len()));
    raw.into_iter()
        .filter(|(h, r)| !(h.is_empty() && body[r.clone()].trim().is_empty()))
        .collect()
}

/// Splice `new_text` into sections addressed by index (from [`section_spans`]),
/// replacing each section's *trimmed content* while preserving the heading line
/// and the surrounding blank lines byte-for-byte. Edits apply back-to-front so
/// byte offsets stay valid across multiple edits to one body.
pub fn apply_section_edits(
    body: &str,
    spans: &[(String, Range<usize>)],
    edits: &[(usize, String)],
) -> String {
    let mut edits: Vec<&(usize, String)> = edits.iter().filter(|(i, _)| *i < spans.len()).collect();
    edits.sort_by(|a, b| spans[b.0].1.start.cmp(&spans[a.0].1.start));
    let mut body = body.to_string();
    for (i, new_text) in edits {
        let range = spans[*i].1.clone();
        let content = &body[range.clone()];
        let trimmed = content.trim();
        if trimmed.is_empty() {
            body.replace_range(range, &format!("\n{new_text}\n"));
        } else {
            let lead = content.find(trimmed).unwrap_or(0);
            let ts = range.start + lead;
            let te = ts + trimmed.len();
            body.replace_range(ts..te, new_text);
        }
    }
    body
}

#[cfg(test)]
mod tests {

    #[test]
    fn section_edit_splices_byte_accurately() {
        let body = "# T\n\nintro\n\n## A\nold a\n\n## B\nold b\n";
        let spans = section_spans(body);
        let a = spans.iter().position(|(h, _)| h == "A").unwrap();
        let out = apply_section_edits(body, &spans, &[(a, "new a".to_string())]);
        // only section A content changes; heading lines, blank lines, B untouched
        assert_eq!(out, "# T\n\nintro\n\n## A\nnew a\n\n## B\nold b\n");
    }
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
