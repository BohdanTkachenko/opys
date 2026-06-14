//! The uniform `references` field: an ID->title map linking a feature or work
//! item to other features and work items.
//!
//! Both document families share one `references` mapping (keys are
//! `FEAT-NNNN` / `WI-NNNN`, values are the referenced doc's title). Prefixes
//! are self-describing, so a single field captures links in either direction.
//! Entries are always serialized sorted by item number. A *closed* work item
//! leaves a tombstone: its title value is struck through (`~~title~~`), which
//! both marks it done and reserves its ID against reuse.

use serde_norway::{Mapping, Value};

use crate::frontmatter::Frontmatter;

/// The reserved frontmatter key holding the ID->title reference map.
pub const FIELD: &str = "references";

/// Numeric part of a `PREFIX-NNNN` id, for deterministic ordering. Ids that do
/// not parse sort last.
pub fn id_number(id: &str) -> u64 {
    id.rsplit_once('-')
        .and_then(|(_, n)| n.parse().ok())
        .unwrap_or(u64::MAX)
}

/// Whether a reference value is a struck-through (closed) tombstone.
pub fn is_struck(value: &str) -> bool {
    let t = value.trim();
    t.len() >= 4 && t.starts_with("~~") && t.ends_with("~~")
}

/// Wrap a title as a struck-through tombstone value.
pub fn strike(title: &str) -> String {
    format!("~~{}~~", title.trim())
}

/// The underlying title of a reference value, with any strikethrough removed.
pub fn unstrike(value: &str) -> &str {
    let t = value.trim();
    if is_struck(t) {
        t[2..t.len() - 2].trim()
    } else {
        t
    }
}

/// Read the `references` map as `(id, raw_value)` pairs sorted by item number.
/// `raw_value` retains any strikethrough so callers can distinguish a closed
/// tombstone from a live link.
pub fn parse(fm: &Frontmatter) -> Vec<(String, String)> {
    let Some(Value::Mapping(m)) = fm.get(FIELD) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = m
        .iter()
        .filter_map(|(k, v)| {
            Some((
                k.as_str()?.to_string(),
                v.as_str().unwrap_or("").to_string(),
            ))
        })
        .collect();
    out.sort_by_key(|e| id_number(&e.0));
    out
}

/// Replace the `references` map, sorted by item number. An empty list removes
/// the field entirely (keeping frontmatter minimal).
pub fn set(fm: &mut Frontmatter, refs: &[(String, String)]) {
    if refs.is_empty() {
        fm.remove(FIELD);
        return;
    }
    let mut sorted = refs.to_vec();
    sorted.sort_by_key(|e| id_number(&e.0));
    let mut m = Mapping::new();
    for (id, title) in sorted {
        m.insert(Value::String(id), Value::String(title));
    }
    fm.insert(FIELD, Value::Mapping(m));
}

/// Ids in the reference map carrying the given prefix (e.g. `FEAT` or `WI`).
pub fn ids_with_prefix(fm: &Frontmatter, prefix: &str) -> Vec<String> {
    let needle = format!("{prefix}-");
    parse(fm)
        .into_iter()
        .map(|(id, _)| id)
        .filter(|id| id.starts_with(&needle))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strike_round_trips() {
        assert!(is_struck("~~done~~"));
        assert!(!is_struck("live"));
        assert_eq!(unstrike("~~done~~"), "done");
        assert_eq!(unstrike("live"), "live");
        assert_eq!(strike("Wire login"), "~~Wire login~~");
    }

    #[test]
    fn set_sorts_by_number_and_round_trips() {
        let mut fm = Frontmatter::new();
        set(
            &mut fm,
            &[
                ("WI-0010".into(), "Ten".into()),
                ("FEAT-0002".into(), "Two".into()),
            ],
        );
        let parsed = parse(&fm);
        assert_eq!(parsed[0].0, "FEAT-0002");
        assert_eq!(parsed[1].0, "WI-0010");
    }

    #[test]
    fn empty_set_removes_field() {
        let mut fm = Frontmatter::new();
        set(&mut fm, &[("WI-0001".into(), "x".into())]);
        assert!(fm.contains_key(FIELD));
        set(&mut fm, &[]);
        assert!(!fm.contains_key(FIELD));
    }
}
