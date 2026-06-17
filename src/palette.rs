//! Configurable presentation rules for the TUI (`[palette]` in opys.toml).
//!
//! Each named entry has a list of `matchers` (`{status?, type?}`) and a `style`.
//! A document matches an entry when any matcher matches (a matcher matches when
//! every field it constrains equals the document's). For a document, the styles
//! of all matching entries are merged field-wise in ascending **specificity**
//! (number of constrained fields in the matched matcher; ties broken by entry
//! name), so more-specific rules override less-specific ones — a `bug` that is
//! `blocked` can take the bug icon *and* the blocked color.
//!
//! This module is part of the core (always compiled): the engine parses and
//! validates the palette even without the `tui` feature. The TUI maps the
//! resolved [`Style`] onto ratatui colors/modifiers; see `tui::theme`.

use std::collections::BTreeMap;

use serde::Deserialize;

/// One palette rule: when any matcher matches, its style contributes.
#[derive(Debug, Clone, Deserialize)]
pub struct PaletteEntry {
    #[serde(default)]
    pub matchers: Vec<Matcher>,
    #[serde(default)]
    pub style: Style,
}

/// A match condition. Both fields optional; an empty matcher matches everything.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Matcher {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default, rename = "type")]
    pub doc_type: Option<String>,
}

/// The presentational attributes a rule can set. All optional so styles compose.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Style {
    #[serde(default)]
    pub fg_color: Option<String>,
    #[serde(default)]
    pub bg_color: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub bold: Option<bool>,
    #[serde(default)]
    pub italic: Option<bool>,
    #[serde(default)]
    pub strikethrough: Option<bool>,
}

impl Style {
    /// Overlay `other` onto `self`: every field `other` sets wins.
    fn overlay(&mut self, other: &Style) {
        if other.fg_color.is_some() {
            self.fg_color = other.fg_color.clone();
        }
        if other.bg_color.is_some() {
            self.bg_color = other.bg_color.clone();
        }
        if other.icon.is_some() {
            self.icon = other.icon.clone();
        }
        if other.bold.is_some() {
            self.bold = other.bold;
        }
        if other.italic.is_some() {
            self.italic = other.italic;
        }
        if other.strikethrough.is_some() {
            self.strikethrough = other.strikethrough;
        }
    }
}

impl Matcher {
    /// Whether this matcher matches the given document type/status, and if so its
    /// specificity (the count of constrained fields). `None` when it does not.
    fn specificity(&self, doc_type: Option<&str>, status: Option<&str>) -> Option<usize> {
        let mut spec = 0;
        if let Some(want) = &self.doc_type {
            if Some(want.as_str()) != doc_type {
                return None;
            }
            spec += 1;
        }
        if let Some(want) = &self.status {
            if Some(want.as_str()) != status {
                return None;
            }
            spec += 1;
        }
        Some(spec)
    }
}

impl PaletteEntry {
    /// The entry's specificity for a document: the highest specificity among its
    /// matching matchers, or `None` if none match.
    fn specificity(&self, doc_type: Option<&str>, status: Option<&str>) -> Option<usize> {
        self.matchers
            .iter()
            .filter_map(|m| m.specificity(doc_type, status))
            .max()
    }
}

/// Resolve the merged [`Style`] for a document from the whole palette.
pub fn resolve(
    palette: &BTreeMap<String, PaletteEntry>,
    doc_type: Option<&str>,
    status: Option<&str>,
) -> Style {
    // (specificity, name, style) for every matching entry.
    let mut matched: Vec<(usize, &str, &Style)> = palette
        .iter()
        .filter_map(|(name, entry)| {
            entry
                .specificity(doc_type, status)
                .map(|spec| (spec, name.as_str(), &entry.style))
        })
        .collect();
    // Least specific first; ties by name. Later (more specific) entries override.
    matched.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));

    let mut out = Style::default();
    for (_, _, style) in matched {
        out.overlay(style);
    }
    out
}

/// A parsed color spec. The TUI maps this onto a concrete ratatui color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpec {
    Rgb(u8, u8, u8),
    Indexed(u8),
    Named(NamedColor),
}

/// The ANSI-ish named colors a palette may reference (the ratatui base set).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Gray,
    DarkGray,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    White,
}

/// Parse a color string: a name (case-insensitive), `#rgb`/`#rrggbb` hex, or a
/// `0`-`255` palette index. Returns `None` if it is not a valid color.
pub fn parse_color(s: &str) -> Option<ColorSpec> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex);
    }
    if let Ok(idx) = s.parse::<u8>() {
        return Some(ColorSpec::Indexed(idx));
    }
    let named = match s.to_ascii_lowercase().as_str() {
        "black" => NamedColor::Black,
        "red" => NamedColor::Red,
        "green" => NamedColor::Green,
        "yellow" => NamedColor::Yellow,
        "blue" => NamedColor::Blue,
        "magenta" => NamedColor::Magenta,
        "cyan" => NamedColor::Cyan,
        "gray" | "grey" => NamedColor::Gray,
        "darkgray" | "darkgrey" => NamedColor::DarkGray,
        "lightred" => NamedColor::LightRed,
        "lightgreen" => NamedColor::LightGreen,
        "lightyellow" => NamedColor::LightYellow,
        "lightblue" => NamedColor::LightBlue,
        "lightmagenta" => NamedColor::LightMagenta,
        "lightcyan" => NamedColor::LightCyan,
        "white" => NamedColor::White,
        _ => return None,
    };
    Some(ColorSpec::Named(named))
}

fn parse_hex(hex: &str) -> Option<ColorSpec> {
    let bytes = match hex.len() {
        3 => {
            let mut full = String::with_capacity(6);
            for c in hex.chars() {
                full.push(c);
                full.push(c);
            }
            return parse_hex(&full);
        }
        6 => hex,
        _ => return None,
    };
    let r = u8::from_str_radix(&bytes[0..2], 16).ok()?;
    let g = u8::from_str_radix(&bytes[2..4], 16).ok()?;
    let b = u8::from_str_radix(&bytes[4..6], 16).ok()?;
    Some(ColorSpec::Rgb(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette(toml: &str) -> BTreeMap<String, PaletteEntry> {
        #[derive(Deserialize)]
        struct Wrap {
            #[serde(default)]
            palette: BTreeMap<String, PaletteEntry>,
        }
        toml::from_str::<Wrap>(toml).unwrap().palette
    }

    #[test]
    fn parses_colors() {
        assert_eq!(parse_color("red"), Some(ColorSpec::Named(NamedColor::Red)));
        assert_eq!(
            parse_color("GREY"),
            Some(ColorSpec::Named(NamedColor::Gray))
        );
        assert_eq!(parse_color("#fff"), Some(ColorSpec::Rgb(255, 255, 255)));
        assert_eq!(parse_color("#112233"), Some(ColorSpec::Rgb(17, 34, 51)));
        assert_eq!(parse_color("200"), Some(ColorSpec::Indexed(200)));
        assert_eq!(parse_color("notacolor"), None);
        assert_eq!(parse_color("#12"), None);
        assert_eq!(parse_color("300"), None); // out of u8 range
    }

    #[test]
    fn specificity_merge_prefers_more_specific() {
        let p = palette(
            "[palette.bug]\nmatchers = [ { type = \"bug\" } ]\n[palette.bug.style]\nicon = \"B\"\nfg_color = \"blue\"\n\
[palette.blocked]\nmatchers = [ { status = \"blocked\" } ]\n[palette.blocked.style]\nfg_color = \"red\"\nbold = true\n\
[palette.bug-blocked]\nmatchers = [ { type = \"bug\", status = \"blocked\" } ]\n[palette.bug-blocked.style]\nfg_color = \"magenta\"\n",
        );
        // A blocked bug: icon from `bug` (B), bold from `blocked`, and fg from the
        // most specific (type+status) rule — magenta.
        let style = resolve(&p, Some("bug"), Some("blocked"));
        assert_eq!(style.icon.as_deref(), Some("B"));
        assert_eq!(style.bold, Some(true));
        assert_eq!(style.fg_color.as_deref(), Some("magenta"));
    }

    #[test]
    fn empty_matcher_matches_everything() {
        let p = palette(
            "[palette.base]\nmatchers = [ {} ]\n[palette.base.style]\nfg_color = \"gray\"\n",
        );
        let style = resolve(&p, Some("feature"), Some("planned"));
        assert_eq!(style.fg_color.as_deref(), Some("gray"));
    }

    #[test]
    fn non_matching_doc_gets_empty_style() {
        let p = palette(
            "[palette.bug]\nmatchers = [ { type = \"bug\" } ]\n[palette.bug.style]\nicon = \"B\"\n",
        );
        let style = resolve(&p, Some("feature"), Some("planned"));
        assert!(style.icon.is_none());
    }
}
