//! The uniform `references` field: an ID->title map linking a feature or work
//! item to other features and work items.
//!
//! Both document families share one `references` mapping (keys are
//! `FEAT-NNNN` / `<TYPE>-NNNN`, values are the referenced doc's title). Prefixes
//! are self-describing, so a single field captures links in either direction.
//! Entries are always serialized sorted by item number. A *closed* work item
//! leaves a tombstone: its title value is struck through (`~~title~~`), which
//! both marks it done and reserves its ID against reuse.

use std::collections::HashMap;

use serde_norway::{Mapping, Value};

use crate::doc::Doc;
use crate::frontmatter::Frontmatter;
use crate::project_config::ProjectConfig;

/// The reserved frontmatter key holding the ID->title reference map.
pub const FIELD: &str = "references";

/// The directional blocker-relation map fields: `blocked_by` lists the items
/// blocking this one, `blocks` lists the items this one blocks. Maintained as
/// inverses of each other by [`crate::links::reconcile_blockers`].
pub const BLOCKED_BY: &str = "blocked_by";
pub const BLOCKS: &str = "blocks";

/// Every ID->title relation map field, in serialization order. Each behaves
/// identically for resolution, tombstones, and ID reservation; only
/// reconciliation differs (`references` is symmetric, `blocked_by`/`blocks`
/// are inverses).
pub const RELATION_FIELDS: [&str; 3] = [FIELD, BLOCKED_BY, BLOCKS];

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
    parse_in(fm, FIELD)
}

/// Replace the `references` map, sorted by item number. An empty list removes
/// the field entirely (keeping frontmatter minimal).
pub fn set(fm: &mut Frontmatter, refs: &[(String, String)]) {
    set_in(fm, FIELD, refs)
}

/// Read an arbitrary ID->title relation map (`references`, `blocked_by`,
/// `blocks`) as `(id, raw_value)` pairs sorted by item number.
pub fn parse_in(fm: &Frontmatter, field: &str) -> Vec<(String, String)> {
    let Some(Value::Mapping(m)) = fm.get(field) else {
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

/// Replace an arbitrary relation map, sorted by item number. An empty list
/// removes the field entirely (keeping frontmatter minimal).
pub fn set_in(fm: &mut Frontmatter, field: &str, refs: &[(String, String)]) {
    if refs.is_empty() {
        fm.remove(field);
        return;
    }
    let mut sorted = refs.to_vec();
    sorted.sort_by_key(|e| id_number(&e.0));
    let mut m = Mapping::new();
    for (id, title) in sorted {
        m.insert(Value::String(id), Value::String(title));
    }
    fm.insert(field, Value::Mapping(m));
}

/// Insert or update `id`'s entry in a relation map. Returns whether the map
/// changed.
pub fn add_to_map(fm: &mut Frontmatter, field: &str, id: &str, title: &str) -> bool {
    let mut entries = parse_in(fm, field);
    if let Some(e) = entries.iter_mut().find(|(i, _)| i == id) {
        if e.1 == title {
            return false;
        }
        e.1 = title.to_string();
    } else {
        entries.push((id.to_string(), title.to_string()));
    }
    set_in(fm, field, &entries);
    true
}

/// Remove `id`'s entry from a relation map. Returns whether anything was removed.
pub fn remove_from_map(fm: &mut Frontmatter, field: &str, id: &str) -> bool {
    let entries = parse_in(fm, field);
    let kept: Vec<_> = entries.iter().filter(|(i, _)| i != id).cloned().collect();
    if kept.len() == entries.len() {
        return false;
    }
    set_in(fm, field, &kept);
    true
}

/// Ids in the `references` map carrying the given prefix (e.g. `FEAT` or `BUG`).
pub fn ids_with_prefix(fm: &Frontmatter, prefix: &str) -> Vec<String> {
    let needle = format!("{prefix}-");
    parse(fm)
        .into_iter()
        .map(|(id, _)| id)
        .filter(|id| id.starts_with(&needle))
        .collect()
}

/// The corpus a blocker question resolves against: the config (for each type's
/// `releases_blocks`) plus every live document's id → status.
///
/// Blocked is derived, never stored (ADR-0050 decision 3), and deriving it needs
/// three things a [`Frontmatter`] cannot supply on its own — which ids still
/// resolve, what status each resolves to, and which statuses that target's type
/// releases at. Bundling them once per command keeps [`live_blockers`] a pure
/// lookup, and keeps the CLI, the board badge and the document view reading the
/// same index rather than three approximations of it.
pub struct Corpus<'a> {
    pcfg: &'a ProjectConfig,
    status: HashMap<&'a str, &'a str>,
}

impl<'a> Corpus<'a> {
    /// Index `(id, status)` pairs — the shape a [`Doc`], a SQL row and the
    /// node's board summary all reduce to.
    pub fn new(
        pcfg: &'a ProjectConfig,
        states: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Self {
        Self {
            pcfg,
            status: states.into_iter().collect(),
        }
    }

    /// The same, over documents the caller has already loaded.
    pub fn of_docs(pcfg: &'a ProjectConfig, docs: &'a [Doc]) -> Self {
        Self::new(
            pcfg,
            docs.iter()
                .filter_map(|d| Some((d.id()?, d.frontmatter.status().unwrap_or_default()))),
        )
    }

    /// Whether any document in the corpus answers to `id`.
    ///
    /// The two ways [`holds`](Corpus::holds) can say "no" are different facts
    /// about the world — a blocker that released, and a blocker that is not
    /// there at all, which `verify` reports as a dangling relation entry — so
    /// anything that explains a non-live blocker to a reader has to tell them
    /// apart.
    pub fn resolves(&self, id: &str) -> bool {
        self.status.contains_key(id)
    }

    /// The status the corpus holds for `id`, when a document answers to it.
    ///
    /// The reading half of [`holds`](Corpus::holds): explaining *why* a target
    /// released needs the status it released at, and re-deriving it from the
    /// documents would be a second index of the one this already keeps.
    pub fn status_of(&self, id: &str) -> Option<&'a str> {
        self.status.get(id).copied()
    }

    /// Whether `id` still holds whatever it blocks: it resolves to a live
    /// document whose status its type does not declare in `releases_blocks`.
    ///
    /// Two "cannot tell" cases, decided in opposite directions on purpose. An id
    /// no document answers to **releases** — there is nothing left to wait for,
    /// and a live entry pointing at nothing is a `verify` finding, not a reason
    /// to hold a document hostage. An id whose prefix matches no configured type
    /// **keeps blocking**: the document is there, and nothing declared that it
    /// releases; a dependency opys cannot evaluate is not a release.
    pub fn holds(&self, id: &str) -> bool {
        let Some(status) = self.status.get(id) else {
            return false;
        };
        self.pcfg
            .type_name_for_id(id)
            .and_then(|name| self.pcfg.types.get(name))
            .is_none_or(|t| !t.releases_blocks.iter().any(|s| s == status))
    }
}

/// The `blocked_by` entries of `doc` that still block it, as `(id, raw_value)`
/// sorted by item number: not struck, and their target [`holds`](Corpus::holds).
///
/// A struck value is a closed document's tombstone — the blocker is gone and the
/// entry survives only to reserve its id — so it is never live.
pub fn live_blockers(doc: &Doc, corpus: &Corpus<'_>) -> Vec<(String, String)> {
    parse_in(&doc.frontmatter, BLOCKED_BY)
        .into_iter()
        .filter(|(id, value)| !is_struck(value) && corpus.holds(id))
        .collect()
}

/// Whether anything live blocks `doc` — the whole of what "blocked" means
/// (ADR-0050 decision 3). There is no blocked status and no blocked flag.
pub fn is_blocked(doc: &Doc, corpus: &Corpus<'_>) -> bool {
    !live_blockers(doc, corpus).is_empty()
}

/// The `blocks` entries of `doc` that it still holds up — the mirror of
/// [`live_blockers`], asked from the blocker's side: an entry counts when it is
/// not struck, its target [`resolves`](Corpus::resolves), and `doc` itself
/// [`holds`](Corpus::holds).
///
/// The third condition is what makes this a mirror rather than a count. A
/// document that has reached one of its type's `releases_blocks` statuses holds
/// nothing up any more, and every entry in its map is dead the moment it gets
/// there — so it must report zero, not the size of a map nobody rewrote. That is
/// exactly the reading [`live_blockers`] gives the other side of the same edge.
///
/// The `blocks` map is maintained as the inverse of `blocked_by` by
/// [`crate::links::reconcile_blockers`], and a missing reverse entry is
/// auto-fixed by sync rather than gated by `verify` — so on a corpus that has
/// not been synced this can under-report. Reading `blocked_by` backwards instead
/// would need the whole corpus rather than one document, which is the trade the
/// relation maps exist to avoid.
pub fn live_blocks(doc: &Doc, corpus: &Corpus<'_>) -> Vec<(String, String)> {
    let Some(id) = doc.id() else {
        return Vec::new();
    };
    if !corpus.holds(id) {
        return Vec::new();
    }
    parse_in(&doc.frontmatter, BLOCKS)
        .into_iter()
        .filter(|(target, value)| !is_struck(value) && corpus.resolves(target))
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

    /// A feature releases what it blocks once it is `implemented`; a task
    /// declares nothing and so blocks at every status it can hold (it releases
    /// by being closed, which strikes the entry).
    fn blocker_cfg() -> ProjectConfig {
        toml::from_str(
            r#"
[types.feature]
prefix = "FEAT"
statuses = ["planned", "implemented"]
default_status = "planned"
releases_blocks = ["implemented"]

[types.task]
prefix = "TASK"
statuses = ["todo", "done"]
default_status = "todo"
terminal_statuses = ["done"]
"#,
        )
        .expect("parses")
    }

    fn doc_with(blocked_by: &str) -> Doc {
        Doc::parse(
            std::path::PathBuf::from("TASK-0002.md"),
            &format!("---\nid: TASK-0002\nstatus: todo\n{blocked_by}---\n\n# Wire it up\n"),
        )
        .expect("parses")
    }

    /// The four shapes an entry can have. Only the last one blocks.
    #[test]
    fn a_blocker_is_live_only_while_its_target_holds() {
        let pcfg = blocker_cfg();
        let corpus = Corpus::new(
            &pcfg,
            [
                ("FEAT-0001", "implemented"),
                ("FEAT-0003", "planned"),
                ("TASK-0004", "todo"),
            ],
        );

        // Struck: the blocker was closed and the tombstone only reserves its id.
        let struck = doc_with("blocked_by:\n  TASK-0004: '~~Ship it~~'\n");
        assert_eq!(live_blockers(&struck, &corpus), vec![]);
        assert!(!is_blocked(&struck, &corpus));

        // Missing: no document answers to the id, so there is nothing to wait
        // for (verify reports the dangling entry separately).
        let missing = doc_with("blocked_by:\n  TASK-0009: Gone\n");
        assert!(!is_blocked(&missing, &corpus));

        // Releasing: the feature reached a status its type releases at.
        let released = doc_with("blocked_by:\n  FEAT-0001: Login\n");
        assert!(!is_blocked(&released, &corpus));

        // Live: the feature is still `planned`.
        let live = doc_with("blocked_by:\n  FEAT-0003: Signup\n");
        assert_eq!(
            live_blockers(&live, &corpus),
            vec![("FEAT-0003".to_string(), "Signup".to_string())]
        );
        assert!(is_blocked(&live, &corpus));
    }

    /// The two non-live cases are distinguishable: a released blocker is still
    /// a document, a dangling one is not. A reader told "it reached a status its
    /// type releases at" about an id nothing answers to is told a fiction, so
    /// the node splits them with this.
    #[test]
    fn a_released_blocker_resolves_and_a_dangling_one_does_not() {
        let pcfg = blocker_cfg();
        let corpus = Corpus::new(&pcfg, [("FEAT-0001", "implemented")]);

        assert!(corpus.resolves("FEAT-0001"));
        assert!(!corpus.holds("FEAT-0001"));

        assert!(!corpus.resolves("TASK-0009"));
        assert!(!corpus.holds("TASK-0009"));
    }

    /// One live entry among released ones is enough, and the live set comes back
    /// sorted by item number like every other relation-map read.
    #[test]
    fn one_live_entry_blocks_and_the_set_is_sorted() {
        let pcfg = blocker_cfg();
        let corpus = Corpus::new(
            &pcfg,
            [
                ("FEAT-0001", "implemented"),
                ("FEAT-0003", "planned"),
                ("TASK-0004", "todo"),
            ],
        );
        let doc = doc_with(
            "blocked_by:\n  TASK-0004: Ship it\n  FEAT-0001: Login\n  FEAT-0003: Signup\n",
        );
        assert_eq!(
            live_blockers(&doc, &corpus),
            vec![
                ("FEAT-0003".to_string(), "Signup".to_string()),
                ("TASK-0004".to_string(), "Ship it".to_string()),
            ]
        );
    }

    /// An id whose prefix matches no configured type is a dependency opys cannot
    /// evaluate, and that is not a release: it keeps blocking.
    #[test]
    fn an_unconfigured_prefix_keeps_blocking() {
        let pcfg = blocker_cfg();
        let corpus = Corpus::new(&pcfg, [("NOTE-0007", "open")]);
        let doc = doc_with("blocked_by:\n  NOTE-0007: A note\n");
        assert!(is_blocked(&doc, &corpus));
    }

    /// A document with no `blocked_by` map at all is never blocked.
    #[test]
    fn no_map_is_not_blocked() {
        let pcfg = blocker_cfg();
        let corpus = Corpus::new(&pcfg, []);
        assert!(!is_blocked(&doc_with(""), &corpus));
    }
}
