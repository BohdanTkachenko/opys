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

/// One entry of a `log` section: a top-level `- ` bullet, its leading UTC
/// timestamp (when it carries one), and the rest of its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// The bullet's leading RFC3339 UTC stamp, verbatim, or `None` when the
    /// bullet is undated — which is what `verify` reports and what the
    /// migration in `docs/migration-history.md` fixes.
    pub ts: Option<String>,
    /// Everything after the stamp: the bullet's first line plus its
    /// continuation lines, joined by newlines with the two-space indent
    /// stripped.
    pub text: String,
}

/// Parse the *content of a `log` section* (what [`section`] returns) into its
/// entries.
///
/// A top-level `- ` bullet starts an entry; every other line — an indented
/// continuation, a blank line, stray prose — belongs to the entry above it, and
/// is dropped when no entry has started yet. That is the same "a line starting
/// with `- ` is an item" rule the `sections` projection counts by, so the two
/// cannot disagree about how many entries a log holds.
///
/// The stamp is recognized structurally: the first whitespace-delimited token,
/// when it is RFC3339 spelled in UTC (`…Z`). Reading is deliberately looser than
/// writing — [`crate::commands::log`] always emits whole seconds — so a stamp
/// with fractional digits still parses. A legacy `- 2026-08-24: …` bullet does
/// *not*: the trailing colon is not part of RFC3339, so it reads as undated,
/// which is exactly what makes it findable.
///
/// Like [`checklist_items`], this is not aware of fenced code blocks.
pub fn log_entries(section: &str) -> Vec<LogEntry> {
    let mut out: Vec<LogEntry> = Vec::new();
    for line in section.lines() {
        if let Some(rest) = line.strip_prefix("- ") {
            let rest = rest.trim_start();
            let (ts, text) = match rest.split_once(char::is_whitespace) {
                Some((tok, tail)) if crate::history::is_utc_key(tok) => {
                    (Some(tok.to_string()), tail.trim_start())
                }
                _ if crate::history::is_utc_key(rest) => (Some(rest.to_string()), ""),
                _ => (None, rest),
            };
            out.push(LogEntry {
                ts,
                text: text.to_string(),
            });
        } else if let Some(last) = out.last_mut() {
            let cont = line.strip_prefix("  ").unwrap_or(line).trim_end();
            last.text.push('\n');
            last.text.push_str(cont);
        }
    }
    for e in &mut out {
        // Blank lines between one entry and the next accumulate onto the one
        // above; they are separators, not content.
        e.text.truncate(e.text.trim_end().len());
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
///
/// An *empty* section is the case worth spelling out, because it is the one
/// [`crate::commands::log`] hits on a freshly scaffolded document. The heading
/// regex's trailing `\s*` has already swallowed the newline after the heading
/// (there is nothing else for it to eat), so an empty span starts on a fresh
/// line and ends either at EOF or just before the blank line that separates the
/// section from the next `## ` heading. The new text therefore goes in *first*,
/// with the original whitespace kept after it — which is what preserves that
/// separating blank line. (Writing `"\n{new}\n"` over the span instead, as this
/// used to, both indented the first entry by a stray blank line and ate the
/// separator.)
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
            let kept = content.to_string();
            // A heading with no newline after it at all (a body ending `## X`)
            // is the only span that does not start on a fresh line.
            let lead = if body[..range.start].ends_with('\n') {
                ""
            } else {
                "\n"
            };
            body.replace_range(range, &format!("{lead}{new_text}\n{kept}"));
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

    /// The three body shapes `opys log` splices into. The empty ones are the
    /// reason the empty branch keeps the original whitespace *after* the new
    /// text: the blank line before the next heading is part of the document.
    #[test]
    fn section_edit_fills_an_empty_section_without_moving_anything_else() {
        for (body, want) in [
            // Empty and last — the freshly scaffolded shape.
            (
                "# T\n\n## Tasks\n- [ ] x\n\n## Progress\n",
                "# T\n\n## Tasks\n- [ ] x\n\n## Progress\n- e\n",
            ),
            // Empty, with another section after it: the separator survives.
            (
                "# T\n\n## Progress\n\n## Notes\nn\n",
                "# T\n\n## Progress\n- e\n\n## Notes\nn\n",
            ),
            // Non-empty: the entry lands under the last one, separator intact.
            (
                "# T\n\n## Progress\n- a\n\n## Notes\nn\n",
                "# T\n\n## Progress\n- a\n- e\n\n## Notes\nn\n",
            ),
        ] {
            let spans = section_spans(body);
            let i = spans.iter().position(|(h, _)| h == "Progress").unwrap();
            let cur = body[spans[i].1.clone()].trim();
            let new_text = if cur.is_empty() {
                "- e".to_string()
            } else {
                format!("{cur}\n- e")
            };
            assert_eq!(apply_section_edits(body, &spans, &[(i, new_text)]), want);
        }
    }

    /// A heading with nothing after it at all — no trailing newline — is the one
    /// empty span that does not start on a fresh line.
    #[test]
    fn section_edit_fills_a_headings_only_tail_section() {
        let body = "# T\n\n## Progress";
        let spans = section_spans(body);
        let i = spans.iter().position(|(h, _)| h == "Progress").unwrap();
        let out = apply_section_edits(body, &spans, &[(i, "- e".to_string())]);
        assert_eq!(out, "# T\n\n## Progress\n- e\n");
    }

    #[test]
    fn log_entries_split_on_top_level_bullets_and_keep_continuations() {
        let section =
            "\n- 2026-01-02T03:04:05Z first\n  wrapped\n\n- 2026-01-02T03:04:06Z second\n";
        let got = log_entries(section);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].ts.as_deref(), Some("2026-01-02T03:04:05Z"));
        assert_eq!(got[0].text, "first\nwrapped");
        assert_eq!(got[1].ts.as_deref(), Some("2026-01-02T03:04:06Z"));
        assert_eq!(got[1].text, "second");
    }

    #[test]
    fn log_entries_report_an_undated_bullet_and_ignore_leading_prose() {
        // Prose before the first bullet belongs to nothing; the legacy
        // `YYYY-MM-DD:` form is undated, which is what the migration greps for.
        let section = "some prose\n- plain note\n- 2026-08-24: legacy\n";
        let got = log_entries(section);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].ts, None);
        assert_eq!(got[0].text, "plain note");
        assert_eq!(got[1].ts, None);
        assert_eq!(got[1].text, "2026-08-24: legacy");
    }

    #[test]
    fn log_entries_accept_a_fractional_stamp_and_a_bare_one() {
        let got =
            log_entries("- 2026-01-02T03:04:05.500000Z with fraction\n- 2026-01-02T03:04:06Z\n");
        assert_eq!(got[0].ts.as_deref(), Some("2026-01-02T03:04:05.500000Z"));
        assert_eq!(got[0].text, "with fraction");
        assert_eq!(got[1].ts.as_deref(), Some("2026-01-02T03:04:06Z"));
        assert_eq!(got[1].text, "");
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
