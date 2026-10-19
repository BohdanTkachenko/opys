//! The lifecycle graph of one project: statuses as nodes, the edges the
//! `transitions` maps declare, and the documents standing on each (FEAT-0060).
//!
//! The board draws a column per status because that is the shape a corpus has
//! on disk. This draws the shape the *config* describes: a node per distinct
//! status name, an edge wherever some type declares the move, and on each node
//! the documents an agent at that stage would pick up next. It is one project's
//! picture, answered from **main** (ADR-0053) — pickability is a property of the
//! truth branch, not of whichever worktree the reader happens to be looking at.
//!
//! Three rules carry the module:
//!
//! - **A node is a status *name*, merged across types.** `task`, `bug` and
//!   `chore` all declare `todo`; drawing three `todo` nodes would be drawing the
//!   config's table of contents rather than its lifecycle. [`FlowNode::types`]
//!   says which types stand behind a merged node, and the guards stay per type —
//!   nothing here evaluates a rule.
//!
//! - **The edges come from [`lifecycle::allowed_targets`] and nowhere else.**
//!   That function is the single reader of a type's `transitions` map, and it is
//!   what encodes the two things a direct read would lose: a type with no map
//!   keeps any-to-any (it contributes its statuses as nodes and *no* edges,
//!   because drawing the complete graph would be a lie about a config that
//!   simply did not say), and a status absent from a non-empty map is a dead end
//!   rather than a wildcard.
//!
//! - **The items are [`next::pick`]'s answer, verbatim.** Not a re-ranking, not
//!   a lighter item struct: the same [`next::Item`] the CLI prints and the
//!   `next` endpoint serves, so the view and `opys next --status <s>` cannot
//!   disagree. That costs one `pick` per status, and each rebuilds the ordering
//!   corpus (`order::Corpus::of_docs`, whose effective-priority precompute walks
//!   the transitive `blocks` closure): O(statuses × corpus). It is paid on a warm
//!   cache, off the hot path, and it is the price of the guarantee — a
//!   hand-rolled ranking here is exactly the drift the API test forbids.
//!
//! Blocked documents are **counted, never listed**: they are not pickable, and a
//! node that listed them would be offering work nobody can take. Orphans come
//! straight off [`next::PickResult::orphaned`], which is complete before any
//! limit, because a work item whose parent shipped will never clear on its own.
//!
//! [`flow`] is a pure function over the documents it is handed — no manager, no
//! actor, no filesystem — for the same reason [`crate::union::union`] is: it is
//! testable without a server, and the actor's only job is to hand it a warm
//! corpus.

use std::collections::BTreeMap;

use opys_engine::doc::Doc;
use opys_engine::next::{self, Item, Orphan, PickQuery};
use opys_engine::project::Project;
use opys_engine::project_config::ProjectConfig;
use opys_engine::{lifecycle, observe, refs};
use serde::Serialize;

/// How many items a node lists when the caller names no `?n=`.
///
/// Deliberately smaller than [`next::DEFAULT_LIMIT`]: a graph node is a card
/// among a dozen others, not a list view, and ten items on each of them is a
/// wall of text with the graph lost inside it. The count of what is behind the
/// cut is on the node ([`FlowNode::pickable`]), and `?n=` raises it.
pub const DEFAULT_NODE_LIMIT: usize = 5;

/// One project's lifecycle graph, with the documents standing on it.
#[derive(Debug, Clone, Serialize)]
pub struct FlowView {
    /// The corpus this was answered from: the project's primary worktree.
    ///
    /// In the payload because every item links to a document, and a document
    /// route is `/corpus/{cid}/doc/{id}`. Deriving it in the browser would be a
    /// second copy of the node's primary-else-first rule, free to disagree with
    /// the corpus the items actually came from.
    pub cid: String,
    /// The per-node item cap this answer was computed with — `items` is
    /// truncated to it, `pickable` is not.
    pub limit: usize,
    /// The declared types, in declaration order (never alphabetical: that is the
    /// order `order::group_of` ranks by, so it is the order the rest of the tool
    /// shows).
    pub types: Vec<FlowType>,
    /// The nodes, in first-appearance order over `types` × each type's declared
    /// statuses, with any status held by a document but declared by nobody last.
    pub nodes: Vec<FlowNode>,
    /// The edges, in first-appearance order, merged across the types that
    /// declare them.
    pub edges: Vec<FlowEdge>,
}

/// One document type, as the graph's legend needs it.
#[derive(Debug, Clone, Serialize)]
pub struct FlowType {
    pub name: String,
    pub prefix: String,
    /// Its statuses, in declared order.
    pub statuses: Vec<String>,
    pub default_status: String,
    pub terminal_statuses: Vec<String>,
    /// Whether this type declares a status graph at all. `false` is any-to-any
    /// — the type contributes nodes and no edges, and a client must not read the
    /// missing edges as "these moves are forbidden".
    pub has_graph: bool,
}

/// One status, with everything standing on it.
#[derive(Debug, Clone, Serialize)]
pub struct FlowNode {
    pub status: String,
    /// The types that declare this status, in declaration order. Empty for an
    /// undeclared status (see [`FlowNode::declared`]).
    pub types: Vec<String>,
    /// Some type's `default_status` — where a new document of it starts, and
    /// what the layout puts in layer 0.
    pub initial: bool,
    /// Listed in some type's `terminal_statuses`.
    ///
    /// A terminal node is *always* empty of ephemeral documents — `close`
    /// deletes the file and `set-status` refuses a terminal target — so it is
    /// marked rather than left looking like a stage nobody reached.
    pub terminal: bool,
    /// Whether any type declares this status.
    ///
    /// `false` means a document is sitting at a status the config does not know
    /// (a hand edit, a bad merge) — including the blank one, where [`status`] is
    /// the empty string and the client supplies the label. `verify` reports it;
    /// the graph shows it, because a node silently dropped here would hide the
    /// document too.
    ///
    /// [`status`]: FlowNode::status
    pub declared: bool,
    /// Documents at this status, with an id. Equals
    /// `pickable + blocked.len() + orphaned.len()`.
    pub total: usize,
    /// How many are pickable, before `limit` truncates `items`.
    pub pickable: usize,
    /// The first `limit` pickable documents, in `opys next` order.
    pub items: Vec<Item>,
    /// The ids of the documents held by a live blocker: counted and greyed,
    /// never listed, because they are not pickable. Ids rather than a bare
    /// count for the same reason [`crate::actor::DocSummary::live_blockers`] is
    /// a list — a count that disagreed with the names would be undebuggable.
    pub blocked: Vec<String>,
    /// The stranded documents at this status, with the reason each is stranded.
    /// Complete, never truncated: [`next::pick`]'s own diagnostic.
    pub orphaned: Vec<Orphan>,
}

/// One declared move, merged across the types that declare it.
#[derive(Debug, Clone, Serialize)]
pub struct FlowEdge {
    pub from: String,
    pub to: String,
    /// The types declaring this move, in declaration order.
    pub types: Vec<String>,
    /// Whether `to` is a terminal status of every type on this edge — the move
    /// `close` makes, which `set-status` refuses.
    pub closing: bool,
}

/// Build one project's flow graph.
///
/// `docs` is the whole corpus: the ordering index and the blocker index are both
/// built over every document, because effective priority and liveness run
/// through documents no status filter would have kept.
pub fn flow(prj: &Project, docs: &[Doc], cid: &str, limit: usize) -> FlowView {
    // One instant for every node, so two nodes cannot disagree about whether the
    // same document is stuck. `observe::now` honours `OPYS_NOW`, which is what
    // makes the whole payload pinnable in a test. A closure rather than a
    // function so the instant's type is never named: `time` is the engine's
    // dependency, not this crate's.
    let now = observe::now();
    let pick_at = |status: &str| {
        next::pick(
            prj,
            docs,
            &PickQuery {
                role: None,
                status: status.to_string(),
                type_filter: None,
                limit,
            },
            now,
        )
    };
    let pcfg = &prj.pcfg;
    let blockers = refs::Corpus::of_docs(pcfg, docs);

    let types = flow_types(pcfg);
    let (statuses, node_types) = declared_statuses(pcfg);
    let edges = flow_edges(pcfg);

    let mut nodes: Vec<FlowNode> = Vec::new();
    for status in &statuses {
        let owners = node_types.get(status).cloned().unwrap_or_default();
        let picked = pick_at(status);
        nodes.push(node(prj, docs, &blockers, status, owners, true, picked));
    }
    // A status no type declares but some document holds. Appended rather than
    // merged into the declared order: it has no place in the config's sequence,
    // and putting it last keeps the declared spine of the graph stable.
    //
    // The blank status (`status: ""`) is *not* filtered out here. It is exactly
    // the case `verify` reports as `invalid status ''`, so it is the one a
    // reader most needs to see, and dropping it would hide the document from
    // the view altogether — including from `total`, which is what the header
    // counts. It gets an ordinary `declared: false` node with an empty name;
    // the client labels it. A document with no `status` key at all is a
    // different thing and stays out: it stands on no status, and every node's
    // items come from `next::pick`, which filters by an actual value, so such a
    // node could only ever be an empty one.
    let mut stray: Vec<String> = Vec::new();
    for doc in docs {
        if doc.id().is_none() {
            continue;
        }
        let Some(status) = doc.status() else { continue };
        if statuses.iter().any(|s| s == status) || stray.iter().any(|s| s == status) {
            continue;
        }
        stray.push(status.to_string());
    }
    for status in &stray {
        let picked = pick_at(status);
        nodes.push(node(
            prj,
            docs,
            &blockers,
            status,
            Vec::new(),
            false,
            picked,
        ));
    }

    FlowView {
        cid: cid.to_string(),
        limit,
        types,
        nodes,
        edges,
    }
}

/// The declared types, in declaration order.
fn flow_types(pcfg: &ProjectConfig) -> Vec<FlowType> {
    // `pcfg.types.names()`, not `&pcfg.types`: iterating the map yields the
    // inner `BTreeMap`'s alphabetical order, and declaration order is what
    // `order::group_of` (and therefore every ranked list) uses.
    pcfg.types
        .names()
        .filter_map(|name| {
            let t = pcfg.types.get(name)?;
            Some(FlowType {
                name: name.to_string(),
                prefix: t.prefix.clone(),
                statuses: t.statuses.clone(),
                default_status: t.default_status.clone(),
                terminal_statuses: t.terminal_statuses.clone(),
                // Asked through `allowed_targets` rather than by looking at the
                // map, so "declares a graph" means exactly what the guards mean
                // by it.
                has_graph: t
                    .statuses
                    .first()
                    .map(|s| lifecycle::allowed_targets(t, s).is_some())
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// Every declared status in first-appearance order, plus which types declare it.
fn declared_statuses(pcfg: &ProjectConfig) -> (Vec<String>, BTreeMap<String, Vec<String>>) {
    let mut order: Vec<String> = Vec::new();
    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for name in pcfg.types.names() {
        let Some(t) = pcfg.types.get(name) else {
            continue;
        };
        for status in &t.statuses {
            if !order.iter().any(|s| s == status) {
                order.push(status.clone());
            }
            let entry = owners.entry(status.clone()).or_default();
            if !entry.iter().any(|n| n == name) {
                entry.push(name.to_string());
            }
        }
    }
    (order, owners)
}

/// Every declared move, merged across types, in first-appearance order.
fn flow_edges(pcfg: &ProjectConfig) -> Vec<FlowEdge> {
    let mut edges: Vec<FlowEdge> = Vec::new();
    for name in pcfg.types.names() {
        let Some(t) = pcfg.types.get(name) else {
            continue;
        };
        for from in &t.statuses {
            // `None` is a type with no map at all: it contributes no edges, and
            // that is the whole any-to-any carve-out.
            let Some(targets) = lifecycle::allowed_targets(t, from) else {
                continue;
            };
            for to in targets {
                let closing = t.terminal_statuses.contains(&to);
                match edges.iter_mut().find(|e| e.from == *from && e.to == to) {
                    Some(edge) => {
                        if !edge.types.iter().any(|n| n == name) {
                            edge.types.push(name.to_string());
                        }
                        // Closing only if it closes for *every* type on the
                        // edge: one type's `done` being terminal says nothing
                        // about another's.
                        edge.closing = edge.closing && closing;
                    }
                    None => edges.push(FlowEdge {
                        from: from.clone(),
                        to,
                        types: vec![name.to_string()],
                        closing,
                    }),
                }
            }
        }
    }
    edges
}

/// One node: the pick oracle's answer at `status`, plus what it dropped.
fn node(
    prj: &Project,
    docs: &[Doc],
    blockers: &refs::Corpus<'_>,
    status: &str,
    types: Vec<String>,
    declared: bool,
    picked: next::PickResult,
) -> FlowNode {
    // The same partition `next::select` runs, in the same precedence — an
    // id-less document is skipped, orphaned is decided *before* blocked, so a
    // document that is both is counted once and as the condition that will not
    // clear by itself. Reverse either and `total` stops equalling the parts,
    // which is what the unit test below pins.
    let mut total = 0usize;
    let mut blocked: Vec<String> = Vec::new();
    for doc in docs {
        let Some(id) = doc.id() else { continue };
        if doc.status() != Some(status) {
            continue;
        }
        total += 1;
        if next::orphan_reason(&prj.pcfg, doc, blockers).is_some() {
            continue;
        }
        if !refs::live_blockers(doc, blockers).is_empty() {
            blocked.push(id.to_string());
        }
    }

    FlowNode {
        status: status.to_string(),
        initial: types.iter().any(|name| {
            prj.pcfg
                .types
                .get(name.as_str())
                .is_some_and(|t| t.default_status == status)
        }),
        terminal: types.iter().any(|name| {
            prj.pcfg
                .types
                .get(name.as_str())
                .is_some_and(|t| t.terminal_statuses.iter().any(|s| s == status))
        }),
        types,
        declared,
        total,
        pickable: picked.total,
        items: picked.items,
        blocked,
        orphaned: picked.orphaned,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const CONFIG: &str = r#"
base = "inv"

[types.task]
prefix = "TASK"
statuses = ["todo", "doing", "done"]
default_status = "todo"
terminal_statuses = ["done"]

[types.task.transitions]
todo = ["doing"]
doing = ["todo", "done"]

# Declared so the default order chain keeps its `priority` key: without an `int`
# field of that name `order::ranked` drops it, and a ranking test over this
# config would be ranking by id.
[types.task.fields.priority]
type = "int"

[types.bug]
prefix = "BUG"
statuses = ["todo", "doing", "fixed"]
default_status = "todo"
terminal_statuses = ["fixed"]

[types.bug.transitions]
todo = ["doing"]
doing = ["fixed"]

[types.bug.fields.priority]
type = "int"

[types.note]
prefix = "NOTE"
statuses = ["open", "closed"]
default_status = "open"
"#;

    fn project(config: &str) -> Project {
        let pcfg: ProjectConfig = toml::from_str(config).expect("parses");
        Project {
            root: PathBuf::from("/p"),
            base: PathBuf::from("/p/inv"),
            pcfg,
        }
    }

    fn doc(name: &str, text: &str) -> Doc {
        Doc::parse(PathBuf::from(format!("/p/inv/{name}.md")), text).expect("parses")
    }

    fn view(prj: &Project, docs: &[Doc]) -> FlowView {
        flow(prj, docs, "cid-1", DEFAULT_NODE_LIMIT)
    }

    fn node_named<'a>(v: &'a FlowView, status: &str) -> &'a FlowNode {
        v.nodes
            .iter()
            .find(|n| n.status == status)
            .unwrap_or_else(|| panic!("no {status} node in {:?}", v.nodes))
    }

    /// Statuses merge by name across types, in first-appearance order over the
    /// types' *declaration* order — the order everything else in the tool ranks
    /// by, not the alphabetical one `&TypeMap` would hand back.
    #[test]
    fn statuses_merge_by_name_in_declaration_order() {
        let prj = project(CONFIG);
        let v = view(&prj, &[]);
        assert_eq!(
            v.types.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["task", "bug", "note"],
        );
        assert_eq!(
            v.nodes
                .iter()
                .map(|n| n.status.as_str())
                .collect::<Vec<_>>(),
            vec!["todo", "doing", "done", "fixed", "open", "closed"],
        );
        assert_eq!(node_named(&v, "todo").types, vec!["task", "bug"]);
        assert!(node_named(&v, "todo").initial);
        assert!(node_named(&v, "done").terminal);
        assert!(!node_named(&v, "doing").initial);
    }

    /// A type with no `transitions` map contributes its statuses and no edges:
    /// any-to-any is not a complete graph, it is the absence of a claim.
    #[test]
    fn a_type_without_transitions_contributes_nodes_and_no_edges() {
        let prj = project(CONFIG);
        let v = view(&prj, &[]);
        assert!(v.nodes.iter().any(|n| n.status == "open"));
        assert!(
            !v.edges.iter().any(|e| e.from == "open" || e.to == "open"),
            "{:?}",
            v.edges
        );
        let note = v.types.iter().find(|t| t.name == "note").expect("note");
        assert!(!note.has_graph);
        assert!(
            v.types
                .iter()
                .find(|t| t.name == "task")
                .expect("task")
                .has_graph
        );
    }

    /// Two types declaring the same move share one edge, and the terminal flag
    /// only survives where the target closes for everyone on it.
    #[test]
    fn edges_merge_across_the_types_that_declare_them() {
        let prj = project(CONFIG);
        let v = view(&prj, &[]);
        let edge = |from: &str, to: &str| {
            v.edges
                .iter()
                .find(|e| e.from == from && e.to == to)
                .unwrap_or_else(|| panic!("no {from}->{to} in {:?}", v.edges))
        };
        assert_eq!(edge("todo", "doing").types, vec!["task", "bug"]);
        assert!(!edge("todo", "doing").closing);
        assert_eq!(edge("doing", "done").types, vec!["task"]);
        assert!(edge("doing", "done").closing);
        assert_eq!(edge("doing", "fixed").types, vec!["bug"]);
        assert!(edge("doing", "fixed").closing);
        // `done` is a dead end: it is a key of no map, which is not a wildcard.
        assert!(!v.edges.iter().any(|e| e.from == "done"), "{:?}", v.edges);
    }

    /// The counting identity, which is the whole guard against this module's
    /// partition drifting from `next::select`'s: every document with an id at a
    /// status is pickable, blocked, or orphaned — exactly once.
    #[test]
    fn every_document_is_pickable_blocked_or_orphaned_exactly_once() {
        let prj = project(CONFIG);
        let docs = vec![
            doc(
                "TASK-0001",
                "---\nid: TASK-0001\nstatus: todo\n---\n\n# One\n",
            ),
            doc(
                "TASK-0002",
                "---\nid: TASK-0002\nstatus: todo\nblocked_by:\n  TASK-0001: One\n---\n\n# Two\n",
            ),
            doc(
                "BUG-0003",
                "---\nid: BUG-0003\nstatus: doing\n---\n\n# Three\n",
            ),
            // No id at all: `next::select` skips it, so the count must too.
            doc("stray", "---\nstatus: todo\n---\n\n# Stray\n"),
        ];
        let v = view(&prj, &docs);
        for n in &v.nodes {
            assert_eq!(
                n.total,
                n.pickable + n.blocked.len() + n.orphaned.len(),
                "node {} does not add up: {n:?}",
                n.status,
            );
        }
        let todo = node_named(&v, "todo");
        assert_eq!(todo.total, 2, "the id-less document is not a document here");
        assert_eq!(todo.pickable, 1);
        assert_eq!(todo.blocked, vec!["TASK-0002".to_string()]);
        assert_eq!(
            todo.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["TASK-0001"],
            "a blocked document is counted, never listed",
        );
    }

    /// `limit` truncates the list and nothing else: `pickable` stays the whole
    /// count, so the view can say how many are behind the cut.
    #[test]
    fn the_limit_truncates_the_items_and_not_the_count() {
        let prj = project(CONFIG);
        let docs: Vec<Doc> = (1..=4)
            .map(|n| {
                doc(
                    &format!("TASK-000{n}"),
                    &format!("---\nid: TASK-000{n}\nstatus: todo\n---\n\n# T{n}\n"),
                )
            })
            .collect();
        let v = flow(&prj, &docs, "cid-1", 2);
        let todo = node_named(&v, "todo");
        assert_eq!(v.limit, 2);
        assert_eq!(todo.pickable, 4);
        assert_eq!(todo.items.len(), 2);
    }

    /// A status the config does not declare still gets a node, marked. Dropping
    /// it would hide the document standing on it, which is the one thing a view
    /// over a broken corpus must not do.
    #[test]
    fn a_status_no_type_declares_is_shown_and_marked() {
        let prj = project(CONFIG);
        let docs = vec![doc(
            "TASK-0001",
            "---\nid: TASK-0001\nstatus: limbo\n---\n\n# One\n",
        )];
        let v = view(&prj, &docs);
        let limbo = node_named(&v, "limbo");
        assert!(!limbo.declared);
        assert!(limbo.types.is_empty());
        assert_eq!(limbo.total, 1);
        assert_eq!(limbo.pickable, 1);
        assert_eq!(
            v.nodes.last().map(|n| n.status.as_str()),
            Some("limbo"),
            "an undeclared status goes after the declared spine",
        );
    }

    /// The blank status is an undeclared status like any other, not a document
    /// to drop. It is the case `verify` reports as `invalid status ''`, so a
    /// filter here would hide from the view precisely the document the view is
    /// most useful for — and would leave the header's document count (the sum
    /// of every node's `total`) short of the corpus.
    #[test]
    fn a_blank_status_gets_a_node_rather_than_swallowing_the_document() {
        let prj = project(CONFIG);
        let docs = vec![
            doc(
                "TASK-0001",
                "---\nid: TASK-0001\nstatus: todo\n---\n\n# One\n",
            ),
            doc(
                "TASK-0002",
                "---\nid: TASK-0002\nstatus: \"\"\n---\n\n# Two\n",
            ),
        ];
        let v = view(&prj, &docs);
        let blank = node_named(&v, "");
        assert!(!blank.declared);
        assert!(blank.types.is_empty());
        assert_eq!(blank.total, 1);
        assert_eq!(blank.pickable, 1);
        assert_eq!(
            blank
                .items
                .iter()
                .map(|i| i.id.as_str())
                .collect::<Vec<_>>(),
            vec!["TASK-0002"],
        );
        assert_eq!(
            v.nodes.iter().map(|n| n.total).sum::<usize>(),
            docs.len(),
            "every document with an id stands on exactly one node",
        );
    }

    /// An orphan is reported under its own key with the reason, and is not
    /// double-counted as blocked even when it is both.
    #[test]
    fn an_orphan_is_reported_rather_than_listed_or_counted_twice() {
        let prj = project(
            r#"
base = "inv"

[types.feature]
prefix = "FEAT"
statuses = ["planned", "implemented"]
default_status = "planned"
releases_blocks = ["implemented"]

[types.task]
prefix = "TASK"
statuses = ["todo"]
default_status = "todo"
requires_link = { to = "feature", min = 1 }
"#,
        );
        let docs = vec![
            doc(
                "FEAT-0001",
                "---\nid: FEAT-0001\nstatus: implemented\n---\n\n# Shipped\n",
            ),
            doc(
                "TASK-0002",
                "---\nid: TASK-0002\nstatus: todo\nblocked_by:\n  FEAT-0001: Shipped\n\
                 references:\n  FEAT-0001: Shipped\n---\n\n# Stranded\n",
            ),
        ];
        let v = view(&prj, &docs);
        let todo = node_named(&v, "todo");
        assert_eq!(todo.total, 1);
        assert_eq!(todo.pickable, 0);
        assert!(todo.blocked.is_empty(), "orphaned wins the precedence");
        assert_eq!(todo.orphaned.len(), 1);
        assert_eq!(todo.orphaned[0].id, "TASK-0002");
        assert!(!todo.orphaned[0].reason.is_empty());
    }

    /// The composition guarantee, at the one place the API fixture cannot show
    /// it: a **merged** node holding several documents of *two* types lists
    /// exactly what [`next::pick`] lists for that status, in that order.
    ///
    /// `todo` is declared by `task` and by `bug`, so this is the case the view
    /// is drawn for — task-block then bug-block, each block by priority, which
    /// is neither id order nor priority order across the whole node. A
    /// hand-rolled ranking inside [`node`] would have to reproduce
    /// `order::group_of` *and* the effective-priority chain to survive it.
    #[test]
    fn a_merged_node_lists_exactly_what_next_lists_in_the_same_order() {
        let prj = project(CONFIG);
        let docs = vec![
            doc(
                "TASK-0001",
                "---\nid: TASK-0001\nstatus: todo\npriority: 3\n---\n\n# T1\n",
            ),
            doc(
                "TASK-0002",
                "---\nid: TASK-0002\nstatus: todo\npriority: 1\n---\n\n# T2\n",
            ),
            doc(
                "TASK-0003",
                "---\nid: TASK-0003\nstatus: todo\npriority: 2\n---\n\n# T3\n",
            ),
            doc(
                "BUG-0004",
                "---\nid: BUG-0004\nstatus: todo\npriority: 2\n---\n\n# B4\n",
            ),
            doc(
                "BUG-0005",
                "---\nid: BUG-0005\nstatus: todo\npriority: 1\n---\n\n# B5\n",
            ),
        ];

        // Spelled out, so the test fails loudly if the ranking the whole tool
        // shares ever changes: `task` is declared before `bug`, so its block
        // comes first, and inside a block the lower priority wins.
        let expected = vec![
            "TASK-0002",
            "TASK-0003",
            "TASK-0001",
            "BUG-0005",
            "BUG-0004",
        ];

        let picked = |limit: usize| {
            next::pick(
                &prj,
                &docs,
                &PickQuery {
                    role: None,
                    status: "todo".to_string(),
                    type_filter: None,
                    limit,
                },
                observe::now(),
            )
        };
        // Every field the view draws, not just the id: position and effective
        // priority are what a re-ranking would get subtly wrong.
        let shape = |items: &[Item]| {
            items
                .iter()
                .map(|i| (i.id.clone(), i.position, i.effective_priority))
                .collect::<Vec<_>>()
        };

        let v = view(&prj, &docs);
        let todo = node_named(&v, "todo");
        assert_eq!(todo.types, vec!["task", "bug"], "the node is a merged one");
        assert_eq!(
            todo.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            expected,
            "not `next` order",
        );
        assert_eq!(
            shape(&todo.items),
            shape(&picked(DEFAULT_NODE_LIMIT).items),
            "the node and `next` disagree at the same status",
        );
        assert_eq!(todo.pickable, 5);

        // And under a cap: the same list, cut, with the count behind it intact.
        let capped = flow(&prj, &docs, "cid-1", 2);
        let todo = node_named(&capped, "todo");
        assert_eq!(shape(&todo.items), shape(&picked(2).items));
        assert_eq!(
            todo.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            expected[..2],
            "a cap keeps the head of the ranking, it does not re-rank",
        );
        assert_eq!(todo.pickable, 5, "the cap truncates, it does not filter");
    }

    /// The payload carries the corpus it was answered from, because every item
    /// link is built from it.
    #[test]
    fn the_view_names_the_corpus_it_answered_from() {
        let prj = project(CONFIG);
        assert_eq!(view(&prj, &[]).cid, "cid-1");
    }
}
