//! Per-document styling. The configurable `[palette]` (resolved in
//! [`crate::palette`]) wins; where it sets nothing, a sensible default applies —
//! an icon per type and a color per status — so a project with no palette still
//! gets a legible, colorful board.

use ratatui::style::{Color, Modifier, Style};

use crate::doc::Doc;
use crate::palette::{self, ColorSpec, NamedColor};
use crate::project::Project;

/// The resolved presentation of a document row.
pub struct DocStyle {
    pub icon: String,
    pub style: Style,
}

pub fn doc_style(prj: &Project, d: &Doc) -> DocStyle {
    let tname = d.id().and_then(|id| prj.pcfg.type_name_for_id(id));
    let status = d.status();
    let resolved = palette::resolve(&prj.pcfg.palette, tname, status, &|t| {
        d.frontmatter.has_tag(t)
    });

    let icon = resolved
        .icon
        .clone()
        .unwrap_or_else(|| default_icon(tname).to_string());

    let fg = resolved
        .fg_color
        .as_deref()
        .and_then(to_color)
        .unwrap_or_else(|| default_status_color(status));

    let mut style = Style::default().fg(fg);
    if let Some(bg) = resolved.bg_color.as_deref().and_then(to_color) {
        style = style.bg(bg);
    }
    if resolved.bold == Some(true) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if resolved.italic == Some(true) {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if resolved.strikethrough == Some(true) {
        style = style.add_modifier(Modifier::CROSSED_OUT);
    }

    DocStyle { icon, style }
}

/// Map a parsed [`ColorSpec`] onto a ratatui color.
fn to_color(s: &str) -> Option<Color> {
    palette::parse_color(s).map(|spec| match spec {
        ColorSpec::Rgb(r, g, b) => Color::Rgb(r, g, b),
        ColorSpec::Indexed(i) => Color::Indexed(i),
        ColorSpec::Named(n) => named(n),
    })
}

fn named(n: NamedColor) -> Color {
    match n {
        NamedColor::Black => Color::Black,
        NamedColor::Red => Color::Red,
        NamedColor::Green => Color::Green,
        NamedColor::Yellow => Color::Yellow,
        NamedColor::Blue => Color::Blue,
        NamedColor::Magenta => Color::Magenta,
        NamedColor::Cyan => Color::Cyan,
        NamedColor::Gray => Color::Gray,
        NamedColor::DarkGray => Color::DarkGray,
        NamedColor::LightRed => Color::LightRed,
        NamedColor::LightGreen => Color::LightGreen,
        NamedColor::LightYellow => Color::LightYellow,
        NamedColor::LightBlue => Color::LightBlue,
        NamedColor::LightMagenta => Color::LightMagenta,
        NamedColor::LightCyan => Color::LightCyan,
        NamedColor::White => Color::White,
    }
}

/// A default glyph per common type name, with a neutral fallback. Plain Unicode
/// (no Nerd Font required) so it renders everywhere.
fn default_icon(t: Option<&str>) -> &'static str {
    match t {
        Some("feature") => "✦",
        Some("bug") => "●",
        Some("task") => "▸",
        Some("chore") => "⚙",
        Some("epic") => "◆",
        Some("adr") => "§",
        Some("risk") => "⚠",
        _ => "•",
    }
}

/// The default color for a status name. Exposed for the stats screen, which has
/// no per-document palette context and just colors status labels.
pub fn status_color_for(status: Option<&str>) -> Color {
    default_status_color(status)
}

/// A default color per common status name. Unknown statuses are neutral gray.
fn default_status_color(s: Option<&str>) -> Color {
    match s {
        Some("done" | "implemented" | "closed" | "complete") => Color::Green,
        Some("blocked") => Color::Red,
        Some("in-progress" | "partial" | "doing") => Color::Yellow,
        Some("wontfix" | "archived" | "cancelled") => Color::DarkGray,
        Some("todo" | "planned" | "backlog" | "open") => Color::Blue,
        _ => Color::Gray,
    }
}
