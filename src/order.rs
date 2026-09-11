//! The ordering chain: which of two documents comes first, and why.
//!
//! A type's `[types.<name>] order_by` (ADR-0099 decision 4) names a chain of
//! tiebreaks. Every consumer that has to put documents in an intentional order —
//! `list --sort` today, `opys next` and the flow view later — reads it *only*
//! through here, so the CLI and the node cannot end up with two orders that
//! disagree about the same corpus.
//!
//! Three rules carry the module:
//!
//! - **The default chain is `["priority", "blocks", "id"]`**, and a type that
//!   declares no `order_by` gets it with the `priority` key **dropped** when the
//!   type declares no `int` field of that name (FEAT-0104: "a type with no
//!   priority field sorts by the rest of its chain"). Priority is an opt-in
//!   declared field ([ADR-0095]) — the engine may not require it of a type that
//!   never asked for ordering. An *explicit* chain naming `priority` on such a
//!   type is a config problem instead, because the author asked for something
//!   the type cannot answer ([`crate::project_config::ProjectConfig::validate`]).
//!
//! - **Unprioritised sorts last.** `Option`'s own `Ord` puts `None` first, which
//!   is the exact opposite, so every `Option` comparison here goes through
//!   [`some_first_asc`]/[`some_first_desc`]. The board says the same thing with
//!   `?? Infinity`; this is that rule, in one place, for everything else.
//!
//! - **Effective priority is transitive.** A document's number is the smallest
//!   over itself and everything it *still* blocks, so a task at 30 holding up one
//!   at 1 sorts as 1. "Still" is [`refs::live_blocks`]: not struck, the target
//!   resolves, and the document itself has not released. The closure is walked
//!   with a visited set, so a `blocks` cycle terminates.
//!
//! [ADR-0095]: priority is an `int` field a type opts into; lower = higher.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use time::OffsetDateTime;

use crate::doc::Doc;
use crate::project_config::{DocType, ProjectConfig};
use crate::refs;

/// The frontmatter field the `priority` key reads — the one [ADR-0095] declares.
/// Nothing else in the engine knows the word; this is the single spelling.
pub const PRIORITY_FIELD: &str = "priority";

/// One tiebreak in a type's `order_by` chain.
///
/// A closed set, parsed straight out of `opys.toml`: an unknown key is a TOML
/// parse error (exit 2 from every command), the same trade
/// [`crate::project_config::SectionKind`] and `CheckScope` already make, and
/// deliberately unlike `ttl`/`releases_blocks`, whose values are strings so a
/// typo is a `validate` problem. A misspelled *order* would silently produce a
/// plausible-looking listing, which is the worse failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderKey {
    /// Effective priority ascending (lower = higher), unprioritised last.
    Priority,
    /// How many live documents this one blocks, descending — unblock the most.
    Blocks,
    /// Item number ascending; an id that does not parse sorts last.
    Id,
    /// `updated` descending (newest first); absent or unparseable sorts last.
    Updated,
}

impl OrderKey {
    /// The spelling used in `opys.toml` and in every message about it.
    pub fn as_str(self) -> &'static str {
        match self {
            OrderKey::Priority => "priority",
            OrderKey::Blocks => "blocks",
            OrderKey::Id => "id",
            OrderKey::Updated => "updated",
        }
    }
}

/// The chain a type gets when it declares none: the most important thing first,
/// then the thing most other work waits on, then the oldest id.
pub const DEFAULT_ORDER_BY: [OrderKey; 3] = [OrderKey::Priority, OrderKey::Blocks, OrderKey::Id];

/// Whether `t` declares the `int` field the [`OrderKey::Priority`] key reads.
///
/// The type matters as much as the name: a `priority` declared as an `enum` (or
/// a string) is not the [ADR-0095] rank and cannot be ordered by.
pub fn has_priority_field(t: &DocType) -> bool {
    t.fields
        .get(PRIORITY_FIELD)
        .is_some_and(|f| f.field_type == crate::config::FieldType::Int)
}

/// The chain to order documents of `t` by: its declared `order_by`, or
/// [`DEFAULT_ORDER_BY`] with the `priority` key dropped when the type declares
/// no `int priority`.
///
/// An empty `order_by` is *not* "no ordering" — it is the same as an absent one.
/// A genuinely empty chain would make `--sort` a no-op that looks like a bug,
/// and `transitions` already spells absent-or-empty the same way.
pub fn chain(t: &DocType) -> Vec<OrderKey> {
    if !t.order_by.is_empty() {
        return t.order_by.clone();
    }
    DEFAULT_ORDER_BY
        .iter()
        .copied()
        .filter(|k| *k != OrderKey::Priority || has_priority_field(t))
        .collect()
}

/// The corpus an ordering question resolves against: [`refs::Corpus`] (which
/// `blocks` entries are live) plus the documents themselves, since ordering also
/// reads each one's `priority` and `updated`.
///
/// Effective priority and the live-`blocks` count are computed once here, per
/// document, rather than inside the comparator: a sort asks the same question
/// O(n log n) times, and the closure walk is O(V+E) each time. Precomputing also
/// keeps the cycle guard honest — each document's walk gets its own visited set,
/// so no partial value computed inside another document's walk is ever reused.
///
/// The walk runs over an adjacency map built once ([`Corpus::edges`]), not over
/// the documents: every root's walk re-visits the same documents, and re-parsing
/// a `blocks` map out of YAML at each visit made the precompute cost far more
/// per visit than the visit itself. Each document's map is parsed exactly once,
/// and so is its own `priority`.
pub struct Corpus<'a> {
    pcfg: &'a ProjectConfig,
    refs: refs::Corpus<'a>,
    effective: HashMap<&'a str, Option<i64>>,
    /// id → the ids it still blocks, [`refs::live_blocks`] resolved against the
    /// documents. Its length is the `blocks` key's count.
    edges: HashMap<&'a str, Vec<&'a str>>,
}

impl<'a> Corpus<'a> {
    /// Index documents the caller has already loaded.
    pub fn of_docs(pcfg: &'a ProjectConfig, docs: &'a [Doc]) -> Self {
        let refs_corpus = refs::Corpus::of_docs(pcfg, docs);
        let by_id: HashMap<&'a str, &'a Doc> =
            docs.iter().filter_map(|d| Some((d.id()?, d))).collect();
        // One parse per document, before any walking: `live_blocks` only keeps
        // targets that resolve, and everything that resolves is a key of
        // `by_id`, so the edge can be held as that borrowed id.
        let mut edges: HashMap<&'a str, Vec<&'a str>> = HashMap::with_capacity(by_id.len());
        let mut own: HashMap<&'a str, Option<i64>> = HashMap::with_capacity(by_id.len());
        for (id, doc) in &by_id {
            own.insert(*id, own_priority(doc));
            edges.insert(
                *id,
                refs::live_blocks(doc, &refs_corpus)
                    .into_iter()
                    .filter_map(|(target, _)| by_id.get_key_value(target.as_str()).map(|(k, _)| *k))
                    .collect(),
            );
        }
        let effective = by_id
            .keys()
            .map(|id| (*id, closure_min(id, &edges, &own)))
            .collect();
        Self {
            pcfg,
            refs: refs_corpus,
            effective,
            edges,
        }
    }

    /// The blocker index underneath, for a caller that also needs
    /// [`refs::is_blocked`] over the same documents.
    pub fn blockers(&self) -> &refs::Corpus<'a> {
        &self.refs
    }

    /// The chain for `doc`'s own type, or [`DEFAULT_ORDER_BY`] when its prefix
    /// matches no configured type (nothing declared how to order it, and it
    /// still has an id).
    pub fn chain_for(&self, doc: &Doc) -> Vec<OrderKey> {
        doc.id()
            .and_then(|id| self.pcfg.type_name_for_id(id))
            .and_then(|name| self.pcfg.types.get(name))
            .map_or_else(|| DEFAULT_ORDER_BY.to_vec(), chain)
    }

    /// How many live documents `doc` blocks — [`refs::live_blocks`], counted.
    pub fn live_blocks_count(&self, doc: &Doc) -> usize {
        doc.id()
            .and_then(|id| self.edges.get(id))
            .map_or_else(|| refs::live_blocks(doc, &self.refs).len(), Vec::len)
    }
}

/// The smallest priority over `doc` and everything it transitively blocks, or
/// `None` when nothing in that closure is prioritised.
///
/// A non-integer `priority` reads as absent: `verify` already reports it as a
/// field-type problem, and ordering is not the place to raise it a second time.
pub fn effective_priority(doc: &Doc, corpus: &Corpus<'_>) -> Option<i64> {
    match doc.id().and_then(|id| corpus.effective.get(id)) {
        Some(v) => *v,
        // A document outside the index (a caller comparing something it has not
        // loaded) still gets an answer, from its own number alone.
        None => own_priority(doc),
    }
}

/// Order `a` before or after `b` by `chain`, resolved against `corpus`.
///
/// The chain is applied in order and the first key that separates them wins.
/// When it is exhausted — a chain that is not total, e.g. `["priority"]` — the
/// lowest item number breaks the tie, so the result is a total order for every
/// chain and callers that pick a single winner ([`crate::commands::list`]'s
/// `--sort`, `next` later) never depend on input order.
pub fn compare(a: &Doc, b: &Doc, chain: &[OrderKey], corpus: &Corpus<'_>) -> Ordering {
    let mut out = Ordering::Equal;
    for key in chain {
        if out != Ordering::Equal {
            break;
        }
        out = match key {
            OrderKey::Priority => {
                some_first_asc(effective_priority(a, corpus), effective_priority(b, corpus))
            }
            OrderKey::Blocks => corpus
                .live_blocks_count(b)
                .cmp(&corpus.live_blocks_count(a)),
            OrderKey::Id => number(a).cmp(&number(b)),
            OrderKey::Updated => some_first_desc(updated(a), updated(b)),
        };
    }
    out.then_with(|| number(a).cmp(&number(b)))
}

/// The document's own `priority`, when it is an integer.
fn own_priority(doc: &Doc) -> Option<i64> {
    doc.frontmatter
        .get(PRIORITY_FIELD)
        .and_then(serde_norway::Value::as_i64)
}

/// The `updated` stamp as an instant. It is RFC3339 with a *local* offset, so
/// the strings do not sort chronologically across offsets — this parses.
fn updated(doc: &Doc) -> Option<OffsetDateTime> {
    doc.frontmatter
        .get_str("updated")
        .and_then(crate::history::parse)
}

fn number(doc: &Doc) -> u64 {
    doc.id().map_or(u64::MAX, refs::id_number)
}

/// Ascending, with `None` after every `Some` — the unprioritised-last rule.
/// `Option`'s derived `Ord` is the other way round, which is why this exists.
fn some_first_asc<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Descending (largest first), still with `None` after every `Some`.
fn some_first_desc<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => b.cmp(&a),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// The smaller of two optional numbers, keeping the number when one is absent
/// (`std::cmp::min(Some(3), None)` is `None`, which would lose it).
fn min_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(v), None) | (None, Some(v)) => Some(v),
        (None, None) => None,
    }
}

/// The minimum own-priority over the live-`blocks` closure of `root`, itself
/// included. A fresh visited set per call: a value computed part-way through
/// another document's walk can be short of a branch that walk had already taken,
/// so nothing here is shared between roots.
fn closure_min<'a>(
    root: &'a str,
    edges: &HashMap<&'a str, Vec<&'a str>>,
    own: &HashMap<&'a str, Option<i64>>,
) -> Option<i64> {
    let mut best = own.get(root).copied().flatten();
    let mut seen: HashSet<&'a str> = HashSet::new();
    seen.insert(root);
    let mut stack: Vec<&'a str> = vec![root];
    while let Some(id) = stack.pop() {
        for next in edges.get(id).into_iter().flatten().copied() {
            if !seen.insert(next) {
                continue;
            }
            best = min_opt(best, own.get(next).copied().flatten());
            stack.push(next);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A `feature` that releases what it blocks at `implemented`, a `task` that
    /// declares the `int priority` field, and a `note` that declares none.
    fn cfg() -> ProjectConfig {
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
[types.task.fields.priority]
type = "int"

[types.note]
prefix = "NOTE"
statuses = ["open"]
default_status = "open"
"#,
        )
        .expect("parses")
    }

    fn doc_type(cfg: &ProjectConfig, name: &str) -> DocType {
        cfg.types.get(name).expect("declared type").clone()
    }

    /// A document with `id`, at `status`, carrying whatever extra frontmatter
    /// lines the case needs (`priority:`, a `blocks:` map, `updated:`).
    fn doc(id: &str, status: &str, extra: &str) -> Doc {
        Doc::parse(
            PathBuf::from(format!("{id}.md")),
            &format!("---\nid: {id}\nstatus: {status}\n{extra}---\n\n# Title of {id}\n"),
        )
        .expect("parses")
    }

    /// The implicit default keeps `priority` only where the type declares the
    /// `int` field; an explicit chain is taken exactly as written.
    #[test]
    fn the_default_chain_drops_priority_on_a_type_without_the_int_field() {
        let cfg = cfg();
        assert_eq!(
            chain(&doc_type(&cfg, "task")),
            vec![OrderKey::Priority, OrderKey::Blocks, OrderKey::Id]
        );
        assert_eq!(
            chain(&doc_type(&cfg, "note")),
            vec![OrderKey::Blocks, OrderKey::Id]
        );
        assert!(has_priority_field(&doc_type(&cfg, "task")));
        assert!(!has_priority_field(&doc_type(&cfg, "note")));
    }

    /// A `priority` of the wrong declared type is not the ADR-0095 rank, so the
    /// default chain drops the key for it too — and an explicit chain naming it
    /// is the config problem `validate` reports.
    #[test]
    fn a_non_int_priority_field_is_not_the_priority_key() {
        let cfg: ProjectConfig = toml::from_str(
            "[types.t]\nprefix = \"T\"\nstatuses = [\"open\"]\ndefault_status = \"open\"\n\
             [types.t.fields.priority]\ntype = \"enum\"\nvalues = [\"low\", \"high\"]\n",
        )
        .expect("parses");
        let t = doc_type(&cfg, "t");
        assert!(!has_priority_field(&t));
        assert_eq!(chain(&t), vec![OrderKey::Blocks, OrderKey::Id]);
    }

    /// An explicit chain is verbatim, empty is the same as absent.
    #[test]
    fn an_explicit_chain_is_taken_as_written() {
        let cfg: ProjectConfig = toml::from_str(
            "[types.t]\nprefix = \"T\"\nstatuses = [\"open\"]\ndefault_status = \"open\"\n\
             order_by = [\"updated\", \"id\"]\n",
        )
        .expect("parses");
        assert_eq!(
            chain(&doc_type(&cfg, "t")),
            vec![OrderKey::Updated, OrderKey::Id]
        );
        let empty: ProjectConfig = toml::from_str(
            "[types.t]\nprefix = \"T\"\nstatuses = [\"open\"]\ndefault_status = \"open\"\n\
             order_by = []\n",
        )
        .expect("parses");
        assert_eq!(
            chain(&doc_type(&empty, "t")),
            vec![OrderKey::Blocks, OrderKey::Id]
        );
    }

    /// The transitive minimum, in the shape ADR-0099 names: a task at 30 that
    /// holds up one at 1 is worth doing as if it were the 1.
    #[test]
    fn a_task_at_30_blocking_one_at_1_sorts_as_1() {
        let cfg = cfg();
        let docs = vec![
            doc(
                "TASK-0001",
                "todo",
                "priority: 30\nblocks:\n  TASK-0002: Urgent\n",
            ),
            doc("TASK-0002", "todo", "priority: 1\n"),
            doc("TASK-0003", "todo", "priority: 20\n"),
        ];
        let corpus = Corpus::of_docs(&cfg, &docs);
        assert_eq!(effective_priority(&docs[0], &corpus), Some(1));
        assert_eq!(effective_priority(&docs[2], &corpus), Some(20));
        // …so it comes before the 20 it would otherwise have lost to.
        assert_eq!(
            compare(&docs[0], &docs[2], &[OrderKey::Priority], &corpus),
            Ordering::Less
        );
    }

    /// Two hops, and a chain that reaches through an unprioritised middleman.
    #[test]
    fn effective_priority_reaches_through_the_whole_closure() {
        let cfg = cfg();
        let docs = vec![
            doc("TASK-0001", "todo", "blocks:\n  TASK-0002: Middle\n"),
            doc("TASK-0002", "todo", "blocks:\n  TASK-0003: Deep\n"),
            doc("TASK-0003", "todo", "priority: 5\n"),
        ];
        let corpus = Corpus::of_docs(&cfg, &docs);
        assert_eq!(effective_priority(&docs[0], &corpus), Some(5));
        assert_eq!(effective_priority(&docs[1], &corpus), Some(5));
    }

    /// A dead entry passes nothing along: struck, dangling, or held by a
    /// document that has itself released.
    #[test]
    fn a_dead_blocks_entry_carries_no_priority() {
        let cfg = cfg();
        let docs = vec![
            // Struck: the target is closed and gone.
            doc(
                "TASK-0001",
                "todo",
                "priority: 30\nblocks:\n  TASK-0004: '~~Urgent~~'\n",
            ),
            // Dangling: no document answers to the id.
            doc(
                "TASK-0002",
                "todo",
                "priority: 30\nblocks:\n  TASK-0009: Vanished\n",
            ),
            // The blocker itself has released, so it holds nothing up.
            doc("FEAT-0003", "implemented", "blocks:\n  TASK-0004: Urgent\n"),
            doc("TASK-0004", "todo", "priority: 1\n"),
        ];
        let corpus = Corpus::of_docs(&cfg, &docs);
        assert_eq!(effective_priority(&docs[0], &corpus), Some(30));
        assert_eq!(effective_priority(&docs[1], &corpus), Some(30));
        assert_eq!(effective_priority(&docs[2], &corpus), None);
        assert_eq!(corpus.live_blocks_count(&docs[2]), 0);
    }

    /// A `priority` that is not an integer reads as absent — the documented
    /// rule (format.md, **Ordering**), and the one a refactor that "helpfully"
    /// parsed the string would break with the rest of the suite still green.
    /// `verify` reports the value as a field-type problem; ordering does not
    /// raise it a second time, and it carries nothing through a closure either.
    #[test]
    fn a_non_integer_priority_value_reads_as_absent() {
        let cfg = cfg();
        let docs = vec![
            doc("TASK-0001", "todo", "priority: high\n"),
            doc("TASK-0002", "todo", "priority: 900\n"),
            doc("TASK-0003", "todo", "blocks:\n  TASK-0001: One\n"),
        ];
        let corpus = Corpus::of_docs(&cfg, &docs);
        assert_eq!(effective_priority(&docs[0], &corpus), None);
        // Unprioritised, so it sorts after the 900 it would beat as a number.
        assert_eq!(
            compare(&docs[0], &docs[1], &[OrderKey::Priority], &corpus),
            Ordering::Greater
        );
        // And a document holding it up inherits nothing from it.
        assert_eq!(effective_priority(&docs[2], &corpus), None);
    }

    /// A `blocks` cycle terminates, and every document in it sees the minimum.
    #[test]
    fn a_blocks_cycle_terminates() {
        let cfg = cfg();
        let docs = vec![
            doc(
                "TASK-0001",
                "todo",
                "priority: 30\nblocks:\n  TASK-0002: Two\n",
            ),
            doc(
                "TASK-0002",
                "todo",
                "priority: 7\nblocks:\n  TASK-0001: One\n",
            ),
        ];
        let corpus = Corpus::of_docs(&cfg, &docs);
        assert_eq!(effective_priority(&docs[0], &corpus), Some(7));
        assert_eq!(effective_priority(&docs[1], &corpus), Some(7));
    }

    /// Every prioritised document comes before every unprioritised one, however
    /// large its number — `Option`'s own order would put the absent one first.
    #[test]
    fn unprioritised_sorts_after_every_prioritised_document() {
        let cfg = cfg();
        let docs = vec![
            doc("TASK-0001", "todo", ""),
            doc("TASK-0002", "todo", "priority: 900\n"),
        ];
        let corpus = Corpus::of_docs(&cfg, &docs);
        assert_eq!(
            compare(&docs[0], &docs[1], &[OrderKey::Priority], &corpus),
            Ordering::Greater
        );
        assert_eq!(
            compare(&docs[1], &docs[0], &[OrderKey::Priority], &corpus),
            Ordering::Less
        );
    }

    /// Each key on its own: priority ascending, blocks descending, id
    /// ascending, updated newest-first with the absent stamp last.
    ///
    /// The two stamps are deliberately in the *opposite* string order to their
    /// chronological one — `2026-01-31T23:00:00-04:00` is `2026-02-01T03:00Z`,
    /// three hours after the `Z` stamp that sorts above it as text — so the
    /// `updated` assertions fail if the offset is ever ignored.
    #[test]
    fn each_key_orders_in_its_own_direction() {
        let cfg = cfg();
        let docs = vec![
            doc(
                "TASK-0001",
                "todo",
                "updated: \"2026-02-01T00:00:00Z\"\npriority: 10\nblocks:\n  TASK-0003: Three\n",
            ),
            doc(
                "TASK-0002",
                "todo",
                "updated: \"2026-01-31T23:00:00-04:00\"\npriority: 20\n",
            ),
            doc("TASK-0003", "todo", "priority: 20\n"),
        ];
        let corpus = Corpus::of_docs(&cfg, &docs);
        let (a, b, c) = (&docs[0], &docs[1], &docs[2]);
        // priority: 10 before 20.
        assert_eq!(
            compare(a, b, &[OrderKey::Priority], &corpus),
            Ordering::Less
        );
        // blocks: the one holding something up first, whatever its priority.
        assert_eq!(compare(a, b, &[OrderKey::Blocks], &corpus), Ordering::Less);
        assert_eq!(corpus.live_blocks_count(a), 1);
        assert_eq!(corpus.live_blocks_count(b), 0);
        // id: lowest number first.
        assert_eq!(compare(b, c, &[OrderKey::Id], &corpus), Ordering::Less);
        // updated: newest first, across offsets, with the absent stamp last.
        // TASK-0002's local stamp is the later instant despite sorting first as
        // a string, so `Greater` here is the parse and not a text comparison.
        assert_eq!(
            compare(a, b, &[OrderKey::Updated], &corpus),
            Ordering::Greater
        );
        assert_eq!(compare(b, c, &[OrderKey::Updated], &corpus), Ordering::Less);
    }

    /// The whole default chain, in the order it falls through: equal effective
    /// priority is decided by the blocks count, and an equal count by the id.
    #[test]
    fn the_chain_falls_through_priority_then_blocks_then_id() {
        let cfg = cfg();
        let docs = vec![
            doc("TASK-0001", "todo", "priority: 10\n"),
            doc(
                "TASK-0002",
                "todo",
                "priority: 10\nblocks:\n  TASK-0003: Three\n",
            ),
            doc("TASK-0003", "todo", "priority: 10\n"),
        ];
        let corpus = Corpus::of_docs(&cfg, &docs);
        let chain = chain(&doc_type(&cfg, "task"));
        let mut order: Vec<&Doc> = docs.iter().collect();
        order.sort_by(|a, b| compare(a, b, &chain, &corpus));
        assert_eq!(
            order.iter().map(|d| d.id().unwrap()).collect::<Vec<_>>(),
            vec!["TASK-0002", "TASK-0001", "TASK-0003"]
        );
    }

    /// A chain that cannot separate two documents still yields one order: the
    /// implicit id tiebreak makes `compare` total whatever the config says.
    #[test]
    fn an_exhausted_chain_falls_back_to_the_id() {
        let cfg = cfg();
        let docs = vec![
            doc("TASK-0002", "todo", "priority: 10\n"),
            doc("TASK-0001", "todo", "priority: 10\n"),
        ];
        let corpus = Corpus::of_docs(&cfg, &docs);
        assert_eq!(
            compare(&docs[0], &docs[1], &[OrderKey::Priority], &corpus),
            Ordering::Greater
        );
        assert_eq!(compare(&docs[0], &docs[1], &[], &corpus), Ordering::Greater);
    }
}
