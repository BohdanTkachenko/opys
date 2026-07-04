//! A lightweight markdown highlighter for the preview pane. Line-based with a
//! small inline tokenizer — not a full CommonMark parser, just enough to make a
//! document pleasant to read: frontmatter, headings, lists, checkboxes, block
//! quotes, fenced code, and inline code / bold / strikethrough / links.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Render a full document (frontmatter + body) into styled lines.
pub fn render(text: &str) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut in_frontmatter = false;
    let mut in_code = false;

    for (n, raw) in text.split('\n').enumerate() {
        let line = raw.trim_end_matches('\r');

        // Frontmatter fence: only the leading `---` block (line 0 opens it).
        if line == "---" && !in_code {
            if n == 0 {
                in_frontmatter = true;
                out.push(rule_line(line));
                continue;
            }
            if in_frontmatter {
                in_frontmatter = false;
                out.push(rule_line(line));
                continue;
            }
            // A `---` in the body is a horizontal rule.
            out.push(rule_line("────────"));
            continue;
        }

        if in_frontmatter {
            out.push(frontmatter_line(line));
            continue;
        }

        // Fenced code blocks.
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            out.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(Color::DarkGray),
            )));
            continue;
        }
        if in_code {
            out.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(Color::Green),
            )));
            continue;
        }

        out.push(body_line(line));
    }
    out
}

fn rule_line(s: &str) -> Line<'static> {
    Line::from(Span::styled(
        s.to_string(),
        Style::default().fg(Color::DarkGray),
    ))
}

fn frontmatter_line(line: &str) -> Line<'static> {
    match line.split_once(':') {
        Some((key, val)) if !key.trim().is_empty() && !key.starts_with(' ') => Line::from(vec![
            Span::styled(key.to_string(), Style::default().fg(Color::Blue)),
            Span::styled(format!(":{val}"), Style::default().fg(Color::Gray)),
        ]),
        _ => Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(Color::Gray),
        )),
    }
}

fn body_line(line: &str) -> Line<'static> {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];

    // Headings.
    if let Some(level) = heading_level(trimmed) {
        let color = match level {
            1 => Color::Cyan,
            2 => Color::LightMagenta,
            3 => Color::Yellow,
            _ => Color::Blue,
        };
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }

    // Block quote.
    if let Some(rest) = trimmed.strip_prefix("> ") {
        let mut spans = vec![Span::styled(
            format!("{indent}▏ "),
            Style::default().fg(Color::DarkGray),
        )];
        spans.extend(inline(
            rest,
            Style::default()
                .add_modifier(Modifier::ITALIC)
                .fg(Color::Gray),
        ));
        return Line::from(spans);
    }

    // Checkbox list items.
    if let Some(rest) =
        strip_checkbox(trimmed, "- [ ] ").or_else(|| strip_checkbox(trimmed, "- [x] "))
    {
        let checked = trimmed.starts_with("- [x] ");
        let (marker, mstyle) = if checked {
            ("✓ ", Style::default().fg(Color::Green))
        } else {
            ("☐ ", Style::default().fg(Color::DarkGray))
        };
        let mut spans = vec![
            Span::raw(indent.to_string()),
            Span::styled(marker.to_string(), mstyle),
        ];
        spans.extend(inline(rest, Style::default()));
        return Line::from(spans);
    }

    // Bullet / ordered list items.
    if let Some(rest) = list_marker(trimmed) {
        let marker_len = trimmed.len() - rest.len();
        let marker = &trimmed[..marker_len];
        let mut spans = vec![
            Span::raw(indent.to_string()),
            Span::styled(marker.to_string(), Style::default().fg(Color::Yellow)),
        ];
        spans.extend(inline(rest, Style::default()));
        return Line::from(spans);
    }

    // Plain paragraph.
    Line::from(inline(line, Style::default()))
}

fn heading_level(s: &str) -> Option<usize> {
    let hashes = s.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && s[hashes..].starts_with(' ') {
        Some(hashes)
    } else {
        None
    }
}

fn strip_checkbox<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.strip_prefix(prefix)
}

/// If `s` begins with a list marker (`- `, `* `, `+ `, or `N. `), return the
/// text after it.
fn list_marker(s: &str) -> Option<&str> {
    for p in ["- ", "* ", "+ "] {
        if let Some(rest) = s.strip_prefix(p) {
            return Some(rest);
        }
    }
    // Ordered: digits then ". ".
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && s[digits..].starts_with(". ") {
        return Some(&s[digits + 2..]);
    }
    None
}

/// Tokenize inline markup into styled spans over a `base` style. Handles
/// `` `code` ``, `**bold**`, `~~strike~~`, and `[text](url)`.
fn inline(s: &str, base: Style) -> Vec<Span<'static>> {
    let chars: Vec<char> = s.chars().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;

    let push_plain = |spans: &mut Vec<Span<'static>>, buf: &mut String| {
        if !buf.is_empty() {
            spans.push(Span::styled(std::mem::take(buf), base));
        }
    };

    while i < chars.len() {
        // Inline code.
        if chars[i] == '`' {
            if let Some(close) = find_char(&chars, i + 1, '`') {
                push_plain(&mut spans, &mut buf);
                let content: String = chars[i + 1..close].iter().collect();
                spans.push(Span::styled(content, Style::default().fg(Color::Green)));
                i = close + 1;
                continue;
            }
        }
        // Bold.
        if is_seq(&chars, i, "**") {
            if let Some(close) = find_seq(&chars, i + 2, "**") {
                push_plain(&mut spans, &mut buf);
                let content: String = chars[i + 2..close].iter().collect();
                spans.push(Span::styled(content, base.add_modifier(Modifier::BOLD)));
                i = close + 2;
                continue;
            }
        }
        // Italic (single `*`; bold `**` is matched above, so this is a lone
        // marker). `_` is left alone so snake_case identifiers survive.
        if chars[i] == '*' {
            if let Some(close) = find_char(&chars, i + 1, '*') {
                if close > i + 1 {
                    push_plain(&mut spans, &mut buf);
                    let content: String = chars[i + 1..close].iter().collect();
                    spans.push(Span::styled(content, base.add_modifier(Modifier::ITALIC)));
                    i = close + 1;
                    continue;
                }
            }
        }
        // Strikethrough (a closed-doc tombstone).
        if is_seq(&chars, i, "~~") {
            if let Some(close) = find_seq(&chars, i + 2, "~~") {
                push_plain(&mut spans, &mut buf);
                let content: String = chars[i + 2..close].iter().collect();
                spans.push(Span::styled(
                    content,
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::CROSSED_OUT),
                ));
                i = close + 2;
                continue;
            }
        }
        // Link: [text](url) — show just the text, colored.
        if chars[i] == '[' {
            if let Some((text, end)) = parse_link(&chars, i) {
                push_plain(&mut spans, &mut buf);
                spans.push(Span::styled(
                    text,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::UNDERLINED),
                ));
                i = end;
                continue;
            }
        }
        buf.push(chars[i]);
        i += 1;
    }
    push_plain(&mut spans, &mut buf);
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base));
    }
    spans
}

fn find_char(chars: &[char], from: usize, c: char) -> Option<usize> {
    (from..chars.len()).find(|&i| chars[i] == c)
}

fn is_seq(chars: &[char], at: usize, seq: &str) -> bool {
    let s: Vec<char> = seq.chars().collect();
    at + s.len() <= chars.len() && chars[at..at + s.len()] == s[..]
}

fn find_seq(chars: &[char], from: usize, seq: &str) -> Option<usize> {
    let s: Vec<char> = seq.chars().collect();
    (from..chars.len().saturating_sub(s.len() - 1)).find(|&i| chars[i..i + s.len()] == s[..])
}

/// Parse `[text](url)` starting at `[`; returns (text, index past the `)`).
fn parse_link(chars: &[char], start: usize) -> Option<(String, usize)> {
    let close_br = find_char(chars, start + 1, ']')?;
    if chars.get(close_br + 1) != Some(&'(') {
        return None;
    }
    let close_paren = find_char(chars, close_br + 2, ')')?;
    let text: String = chars[start + 1..close_br].iter().collect();
    Some((text, close_paren + 1))
}

/// Highlight one line for the **editor** — non-destructive: every character is
/// preserved (delimiters included) so the cursor column stays aligned. `in_code`
/// marks a line inside a fenced block. (The preview's [`render`] rewrites text,
/// which would misalign an editable cursor, so the editor uses this instead.)
pub fn highlight_edit_line(line: &str, in_code: bool) -> Line<'static> {
    if line.trim_start().starts_with("```") {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if in_code {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(Color::Green),
        ));
    }

    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];

    if let Some(level) = heading_level(trimmed) {
        let color = match level {
            1 => Color::Cyan,
            2 => Color::LightMagenta,
            3 => Color::Yellow,
            _ => Color::Blue,
        };
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }

    if let Some(rest) = trimmed.strip_prefix("> ") {
        let mut spans = vec![Span::styled(
            format!("{indent}> "),
            Style::default().fg(Color::DarkGray),
        )];
        spans.extend(inline_keep(
            rest,
            Style::default()
                .add_modifier(Modifier::ITALIC)
                .fg(Color::Gray),
        ));
        return Line::from(spans);
    }

    // Checkbox: keep the literal `- [x] ` / `- [ ] `, just color the box.
    if trimmed.starts_with("- [ ] ") || trimmed.starts_with("- [x] ") {
        let checked = trimmed.starts_with("- [x] ");
        let box_style = if checked {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let mut spans = vec![
            Span::raw(indent.to_string()),
            Span::styled("- ".to_string(), Style::default().fg(Color::Yellow)),
            Span::styled(trimmed[2..6].to_string(), box_style),
        ];
        spans.extend(inline_keep(&trimmed[6..], Style::default()));
        return Line::from(spans);
    }

    if let Some(rest) = list_marker(trimmed) {
        let marker = &trimmed[..trimmed.len() - rest.len()];
        let mut spans = vec![
            Span::raw(indent.to_string()),
            Span::styled(marker.to_string(), Style::default().fg(Color::Yellow)),
        ];
        spans.extend(inline_keep(rest, Style::default()));
        return Line::from(spans);
    }

    let mut spans = vec![Span::raw(indent.to_string())];
    spans.extend(inline_keep(trimmed, Style::default()));
    Line::from(spans)
}

/// Inline highlighter that **keeps all characters** (markers included), for the
/// editor. Colors code/bold/italic/strike spans and the two halves of a link.
fn inline_keep(s: &str, base: Style) -> Vec<Span<'static>> {
    let chars: Vec<char> = s.chars().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;

    let flush = |spans: &mut Vec<Span<'static>>, buf: &mut String| {
        if !buf.is_empty() {
            spans.push(Span::styled(std::mem::take(buf), base));
        }
    };

    while i < chars.len() {
        if chars[i] == '`' {
            if let Some(close) = find_char(&chars, i + 1, '`') {
                flush(&mut spans, &mut buf);
                spans.push(Span::styled(
                    chars[i..=close].iter().collect::<String>(),
                    Style::default().fg(Color::Green),
                ));
                i = close + 1;
                continue;
            }
        }
        if is_seq(&chars, i, "**") {
            if let Some(close) = find_seq(&chars, i + 2, "**") {
                flush(&mut spans, &mut buf);
                spans.push(Span::styled(
                    chars[i..close + 2].iter().collect::<String>(),
                    base.add_modifier(Modifier::BOLD),
                ));
                i = close + 2;
                continue;
            }
        }
        if chars[i] == '*' {
            if let Some(close) = find_char(&chars, i + 1, '*') {
                if close > i + 1 {
                    flush(&mut spans, &mut buf);
                    spans.push(Span::styled(
                        chars[i..=close].iter().collect::<String>(),
                        base.add_modifier(Modifier::ITALIC),
                    ));
                    i = close + 1;
                    continue;
                }
            }
        }
        if is_seq(&chars, i, "~~") {
            if let Some(close) = find_seq(&chars, i + 2, "~~") {
                flush(&mut spans, &mut buf);
                spans.push(Span::styled(
                    chars[i..close + 2].iter().collect::<String>(),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::CROSSED_OUT),
                ));
                i = close + 2;
                continue;
            }
        }
        if chars[i] == '[' {
            if let Some(close_br) = find_char(&chars, i + 1, ']') {
                if chars.get(close_br + 1) == Some(&'(') {
                    if let Some(close_paren) = find_char(&chars, close_br + 2, ')') {
                        flush(&mut spans, &mut buf);
                        spans.push(Span::styled(
                            chars[i..=close_br].iter().collect::<String>(),
                            Style::default().fg(Color::Cyan),
                        ));
                        spans.push(Span::styled(
                            chars[close_br + 1..=close_paren].iter().collect::<String>(),
                            Style::default().fg(Color::DarkGray),
                        ));
                        i = close_paren + 1;
                        continue;
                    }
                }
            }
        }
        buf.push(chars[i]);
        i += 1;
    }
    flush(&mut spans, &mut buf);
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn highlights_structure_without_losing_text() {
        let doc = "---\nid: FEAT-0001\nstatus: planned\n---\n\n# Title\n\n- [x] done\n- [ ] todo\n\nSee `mod::test` and [FEAT-0002](x).\n";
        let lines = render(doc);
        let text: Vec<String> = lines.iter().map(plain).collect();
        assert!(text.iter().any(|l| l.contains("id")));
        assert!(text.iter().any(|l| l == "# Title"));
        // Checkbox markers are rewritten to glyphs but keep the item text.
        assert!(text.iter().any(|l| l.contains("done")));
        assert!(text.iter().any(|l| l.contains("todo")));
        // Inline link shows the text, not the url.
        let last = text.iter().find(|l| l.contains("FEAT-0002")).unwrap();
        assert!(!last.contains("(x)"), "url leaked: {last}");
        assert!(last.contains("mod::test"));
    }

    #[test]
    fn heading_levels() {
        assert_eq!(heading_level("# h"), Some(1));
        assert_eq!(heading_level("### h"), Some(3));
        assert_eq!(heading_level("#nospace"), None);
        assert_eq!(heading_level("####### too many"), None);
    }

    #[test]
    fn inline_bold_italic_and_code() {
        let spans = inline("a **b** *i* `c`", Style::default());
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "a b i c");
    }

    #[test]
    fn leaves_snake_case_alone() {
        let spans = inline("call mod::persist_test now", Style::default());
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "call mod::persist_test now");
    }

    #[test]
    fn editor_highlight_preserves_every_character() {
        // The cursor maps by column, so the rendered spans must concatenate back
        // to the exact input line for every markup shape.
        for line in [
            "# Heading",
            "## Test plan",
            "- [x] done `mod::test` and **bold**",
            "- [ ] todo with [FEAT-2](path.md)",
            "> a *quote* with ~~strike~~",
            "  - nested bullet",
            "1. ordered item",
            "plain text, no markup",
        ] {
            let rendered: String = highlight_edit_line(line, false)
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            assert_eq!(rendered, line, "character drift on {line:?}");
        }
    }
}
