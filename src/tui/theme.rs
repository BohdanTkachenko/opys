//! Per-document styling. Phase 1 derives a sensible default icon per type and
//! color per status; Phase 2 will replace this with the configurable `[palette]`
//! while keeping this as the fallback for documents no rule matches.

use ratatui::style::Color;

use crate::doc::Doc;
use crate::project::Project;

/// The resolved presentation of a document row.
pub struct DocStyle {
    pub icon: &'static str,
    pub fg: Color,
}

pub fn doc_style(prj: &Project, d: &Doc) -> DocStyle {
    let tname = d.id().and_then(|id| prj.pcfg.type_name_for_id(id));
    DocStyle {
        icon: type_icon(tname),
        fg: status_color(d.status()),
    }
}

/// A default glyph per common type name, with a neutral fallback. Plain Unicode
/// (no Nerd Font required) so it renders everywhere.
fn type_icon(t: Option<&str>) -> &'static str {
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

/// A default color per common status name. Unknown statuses are neutral gray.
fn status_color(s: Option<&str>) -> Color {
    match s {
        Some("done" | "implemented" | "closed" | "complete") => Color::Green,
        Some("blocked") => Color::Red,
        Some("in-progress" | "partial" | "doing") => Color::Yellow,
        Some("wontfix" | "archived" | "cancelled") => Color::DarkGray,
        Some("todo" | "planned" | "backlog" | "open") => Color::Blue,
        _ => Color::Gray,
    }
}
