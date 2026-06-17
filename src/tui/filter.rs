//! The board filter — the same dimensions as `opys list` (type / status / tag)
//! plus a free-text query over id and title. Applied live as the user edits it.

use crate::doc::Doc;
use crate::project::Project;

#[derive(Default)]
pub struct FilterState {
    pub doc_type: Option<String>,
    pub status: Option<String>,
    pub tag: Option<String>,
    pub query: String,
}

/// Which filter field the filter panel currently edits.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FilterField {
    Type,
    Status,
    Tag,
    Query,
}

impl FilterField {
    pub const ALL: [FilterField; 4] = [
        FilterField::Type,
        FilterField::Status,
        FilterField::Tag,
        FilterField::Query,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FilterField::Type => "type",
            FilterField::Status => "status",
            FilterField::Tag => "tag",
            FilterField::Query => "query",
        }
    }
}

impl FilterState {
    pub fn is_active(&self) -> bool {
        self.doc_type.is_some()
            || self.status.is_some()
            || self.tag_value().is_some()
            || !self.query.trim().is_empty()
    }

    /// The tag filter as a non-empty value (an emptied text field counts as
    /// unset, not "match the empty tag").
    fn tag_value(&self) -> Option<&str> {
        self.tag.as_deref().filter(|t| !t.is_empty())
    }

    pub fn clear(&mut self) {
        *self = FilterState::default();
    }

    /// Whether `d` passes every set filter (AND across dimensions).
    pub fn matches(&self, prj: &Project, d: &Doc) -> bool {
        if let Some(t) = &self.doc_type {
            if d.id().and_then(|id| prj.pcfg.type_name_for_id(id)) != Some(t.as_str()) {
                return false;
            }
        }
        if let Some(s) = &self.status {
            if d.status() != Some(s.as_str()) {
                return false;
            }
        }
        if let Some(tag) = self.tag_value() {
            let has = d
                .frontmatter
                .tags()
                .unwrap_or_default()
                .iter()
                .any(|x| x == tag);
            if !has {
                return false;
            }
        }
        let q = self.query.trim().to_lowercase();
        if !q.is_empty() {
            let hay = format!("{} {}", d.id().unwrap_or(""), d.title).to_lowercase();
            if !hay.contains(&q) {
                return false;
            }
        }
        true
    }

    /// A compact human-readable summary of the active filters (for the bar).
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(t) = &self.doc_type {
            parts.push(format!("type={t}"));
        }
        if let Some(s) = &self.status {
            parts.push(format!("status={s}"));
        }
        if let Some(t) = self.tag_value() {
            parts.push(format!("tag={t}"));
        }
        let q = self.query.trim();
        if !q.is_empty() {
            parts.push(format!("query={q:?}"));
        }
        if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join("  ")
        }
    }
}

/// The selectable type names, sorted.
pub fn type_options(prj: &Project) -> Vec<String> {
    prj.pcfg.types.keys().cloned().collect()
}

/// The selectable statuses: those of the chosen type, or the union across types.
pub fn status_options(prj: &Project, doc_type: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = match doc_type.and_then(|t| prj.pcfg.types.get(t)) {
        Some(t) => t.statuses.clone(),
        None => {
            let mut all: Vec<String> = prj
                .pcfg
                .types
                .values()
                .flat_map(|t| t.statuses.iter().cloned())
                .collect();
            all.sort();
            all.dedup();
            all
        }
    };
    out.retain(|s| !s.is_empty());
    out
}

/// Cycle an optional choice through `None -> options[0] -> ... -> None`. `step`
/// is +1 (right) or -1 (left).
pub fn cycle(current: &Option<String>, options: &[String], step: i32) -> Option<String> {
    if options.is_empty() {
        return None;
    }
    // Index space: 0 == None, 1..=len == options[i-1].
    let len = options.len() as i32;
    let cur = match current {
        None => 0,
        Some(v) => options
            .iter()
            .position(|o| o == v)
            .map(|i| i as i32 + 1)
            .unwrap_or(0),
    };
    let next = (cur + step).rem_euclid(len + 1);
    if next == 0 {
        None
    } else {
        Some(options[(next - 1) as usize].clone())
    }
}
