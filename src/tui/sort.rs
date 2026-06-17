//! Sorting the board. The default is most-recently-updated first, falling back
//! to file mtime for documents that predate the `updated` field.

use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::doc::Doc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Updated,
    Created,
    Id,
    Title,
    Status,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Updated => "updated",
            SortKey::Created => "created",
            SortKey::Id => "id",
            SortKey::Title => "title",
            SortKey::Status => "status",
        }
    }
}

#[derive(Clone, Copy)]
pub struct SortState {
    pub key: SortKey,
    pub desc: bool,
}

impl Default for SortState {
    fn default() -> Self {
        SortState {
            key: SortKey::Updated,
            desc: true,
        }
    }
}

/// Sort `docs` in place according to `state`.
pub fn sort_docs(docs: &mut [Doc], state: SortState) {
    match state.key {
        SortKey::Updated => docs.sort_by_key(|d| time_key(d, "updated")),
        SortKey::Created => docs.sort_by_key(|d| time_key(d, "created")),
        SortKey::Id => docs.sort_by_key(id_number),
        SortKey::Title => docs.sort_by_key(|d| d.title.to_lowercase()),
        SortKey::Status => docs.sort_by(|a, b| status_of(a).cmp(status_of(b))),
    }
    if state.desc {
        docs.reverse();
    }
}

/// The sort timestamp for `field`: the parsed RFC3339 value, else the file's
/// mtime, else 0 (so undated docs sort oldest — last under the default desc).
fn time_key(d: &Doc, field: &str) -> i64 {
    d.frontmatter
        .get_str(field)
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        .or_else(|| mtime(d))
        .map(|t| t.unix_timestamp())
        .unwrap_or(0)
}

fn mtime(d: &Doc) -> Option<OffsetDateTime> {
    let modified = std::fs::metadata(&d.path).ok()?.modified().ok()?;
    Some(OffsetDateTime::from(modified))
}

/// The numeric part of a doc's id (`FEAT-0042` -> 42), for id-ordered sorts.
fn id_number(d: &Doc) -> u64 {
    d.id()
        .and_then(|id| id.rsplit('-').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

fn status_of(d: &Doc) -> &str {
    d.status().unwrap_or("")
}
