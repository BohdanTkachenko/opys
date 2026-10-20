//! `opys next` — the pick oracle: which documents at a status are pickable,
//! and in what order.
//!
//! One deterministic question, answered from the inventory alone: *given a
//! status, what may be picked up now, best first*. It decides nothing about who
//! picks it up. Concurrency caps, occupancy rules, harness affinity, budgets and
//! leases are **policy**, and policy belongs to whatever wraps this (FEAT-0057's
//! "Scope"); opys reports and orders, and something outside it
//! chooses.
//!
//! Three rules carry the module:
//!
//! - **Pickable is three conditions, in this precedence**: the document is at
//!   the queried status (and of the queried type, when one is given); it is not
//!   [orphaned](orphan_reason) — its type's `requires_link` is still satisfied by
//!   targets that have not released; and nothing live blocks it
//!   ([`refs::live_blockers`]). A blocked document is dropped silently, because a
//!   blocker is a normal, temporary state of a healthy corpus. An orphaned one is
//!   *reported*, under its own key with the reason, because a work item whose
//!   parent has been shipped or abandoned will never become pickable on its own.
//!   Orphaned is checked first, so a document that is both is reported once, as
//!   the condition that will not clear by itself.
//!
//! - **The order is [`order`]'s, never a second one.** [`order::Corpus::ranked`]
//!   is the single ranking `list --sort` also calls, so `next` and `list --sort`
//!   cannot disagree about the same corpus. The ordering corpus is built over
//!   **every** document, before any filtering: effective priority walks the
//!   transitive `blocks` closure, and that chain can run through a document the
//!   status or type filter left out.
//!
//! - **Main supplies release decisions.** Declared work branches supply stage
//!   copies through [`work::effective`]; a blocker closed on a branch still
//!   releases nothing until the merge lands. With no declarations no git is read.
//!
//! Nothing here writes or syncs. [`pick_effective`] and [`explain_effective`]
//! share a caller-supplied snapshot; [`pick`] and [`explain`] resolve one first.

use std::collections::HashMap;

use serde::Serialize;
use time::OffsetDateTime;

use crate::doc::Doc;
use crate::error::Result;
use crate::observe::{self, Observation};
use crate::order;
use crate::project::Project;
use crate::project_config::ProjectConfig;
use crate::refs;
use crate::roles;
use crate::work;

/// The value of the payload's `schema` key: the contract a consumer pins.
///
/// A literal, deliberately *not* the crate version: the shape is what a
/// dispatcher parses, and it does not change because opys had a patch release.
/// Fields may be added within `opys.next/1`; removing or repurposing one is what
/// the number is for.
pub const SCHEMA: &str = "opys.next/1";

/// How many items `next` lists when the caller names no limit.
pub const DEFAULT_LIMIT: usize = 10;

/// What to pick: a status, optionally one type, and how many items to list.
///
/// `limit` truncates `items` only. `orphaned` is a diagnostic and is always
/// complete — a caller that asked for one item still wants to know that six of
/// its work items are stranded under a `wontfix` feature.
#[derive(Debug, Clone)]
pub struct PickQuery {
    pub status: String,
    pub type_filter: Option<String>,
    pub role: Option<String>,
    pub limit: usize,
}

impl PickQuery {
    /// Check the query names things the project declares.
    ///
    /// An oracle that answers "nothing" to a misspelled status is a dispatcher
    /// trap: the caller cannot tell "no work" from "no such status". So this is
    /// refused rather than answered — unlike `list`, whose filters are a slice of
    /// whatever happens to be there. An *empty* result for a status that exists
    /// stays a perfectly good answer (exit 0, empty list).
    ///
    /// The `Err` is a plain message so both surfaces can class it themselves: the
    /// CLI raises it as a usage error (exit 2), the node returns it as a 400.
    pub fn validate(&self, pcfg: &ProjectConfig) -> std::result::Result<(), String> {
        if let Some(role) = &self.role {
            roles::validate_role(pcfg, role)?;
        }
        if let Some(name) = &self.type_filter {
            let Some(t) = pcfg.types.get(name) else {
                let mut names: Vec<&str> = pcfg.types.keys().map(String::as_str).collect();
                names.sort_unstable();
                return Err(format!(
                    "unknown type {name:?} (configured: {})",
                    names.join(", ")
                ));
            };
            if !t.statuses.contains(&self.status) {
                return Err(format!(
                    "unknown status {:?} for type '{name}' (allowed: {})",
                    self.status,
                    t.statuses.join(", ")
                ));
            }
            return Ok(());
        }
        if pcfg
            .types
            .values()
            .any(|t| t.statuses.contains(&self.status))
        {
            return Ok(());
        }
        let mut known: Vec<&str> = pcfg
            .types
            .values()
            .flat_map(|t| t.statuses.iter().map(String::as_str))
            .collect();
        known.sort_unstable();
        known.dedup();
        Err(format!(
            "unknown status {:?} (no configured type declares it; known: {})",
            self.status,
            known.join(", ")
        ))
    }
}

/// One pickable document, with the numbers that put it where it is.
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub id: String,
    /// The type its id prefix names, or `null` when no configured type claims
    /// the prefix (`verify` reports that separately; ordering still works).
    #[serde(rename = "type")]
    pub type_name: Option<String>,
    pub title: String,
    pub status: String,
    /// The document's file, **relative to the project root** — the same path on
    /// every machine, which is what makes the payload comparable and pinnable.
    pub path: String,
    pub tags: Vec<String>,
    /// The document's own `priority` field, when it declares an integer one.
    pub priority: Option<i64>,
    /// [`order::effective_priority`]: the smallest priority over this document
    /// and everything it still blocks. Different from `priority` means inherited.
    pub effective_priority: Option<i64>,
    /// How many live documents this one blocks.
    pub blocks: usize,
    /// 1-based rank among *all* pickable documents, before the limit.
    pub position: usize,
    /// This document's [time observations](crate::observe), when its type
    /// configures any: a top item that has been sitting untouched is exactly
    /// what a dispatcher needs to see. Reported here; `verify --fix` is what
    /// acts on them.
    pub observations: Vec<Observation>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub owners: Vec<String>,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claim: Option<work::Claim>,
}

/// A document at the queried status that will not become pickable on its own:
/// its type's `requires_link` is no longer satisfied.
#[derive(Debug, Clone, Serialize)]
pub struct Orphan {
    pub id: String,
    #[serde(rename = "type")]
    pub type_name: Option<String>,
    pub title: String,
    pub status: String,
    pub path: String,
    /// Why, naming each target and how it failed — released at a status its type
    /// declares in `releases_blocks`, or resolving to no document at all.
    pub reason: String,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// The answer: what to pick, what is stranded, and — with `--explain <ID>` — why
/// one named document is or is not on the list.
#[derive(Debug, Clone, Serialize)]
pub struct PickResult {
    /// Always [`SCHEMA`].
    pub schema: &'static str,
    /// The query, echoed, so a stored payload is self-describing.
    pub status: String,
    #[serde(rename = "type")]
    pub type_filter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub limit: usize,
    /// How many documents are pickable in all, before `limit` truncated `items`.
    pub total: usize,
    pub items: Vec<Item>,
    pub orphaned: Vec<Orphan>,
    /// Only present when the caller named a document with `--explain <ID>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explain: Option<Explanation>,
    /// Declared occupancy and claims, including for bare `--explain`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unavailable: Vec<Explanation>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<String>,
}

/// Why one named document is, or is not, on the list.
#[derive(Debug, Clone, Serialize)]
pub struct Explanation {
    pub id: String,
    pub listed: bool,
    /// The line the CLI prints, verbatim.
    pub message: String,
    #[serde(flatten)]
    pub why: Why,
}

/// The reason behind an [`Explanation`], as its own tagged variant so a
/// dispatcher can branch on `reason` rather than parse `message`.
///
/// The variants are the pickability precedence, in order.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum Why {
    Occupied {
        #[serde(rename = "ref")]
        reference: String,
    },
    Claimed {
        claim: work::Claim,
        #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
    },
    /// Not of the queried type.
    OtherType {
        #[serde(rename = "type")]
        type_name: Option<String>,
    },
    /// Not at the queried status.
    OtherStatus { status: String },
    /// Waits on a different role (or nobody).
    OtherRole {
        #[serde(skip_serializing_if = "Vec::is_empty")]
        owners: Vec<String>,
    },
    /// Its `requires_link` no longer holds — see [`Orphan::reason`].
    Orphaned { detail: String },
    /// Something live blocks it.
    Blocked { blockers: Vec<String> },
    /// Pickable, but the limit stopped short of it.
    Behind { position: usize, total: usize },
    /// On the list, at `position` of `total`.
    Listed { position: usize, total: usize },
}

/// Everything one pass over the corpus produces: the ordering index, the ranked
/// pickable set (whole, before any limit), the ranked orphans and their reasons.
pub(crate) struct Selection<'a> {
    pub(crate) corpus: order::Corpus<'a>,
    pub(crate) pickable: Vec<&'a Doc>,
    pub(crate) blocked: Vec<&'a Doc>,
    pub(crate) claimed: Vec<&'a Doc>,
    pub(crate) orphaned: Vec<&'a Doc>,
    pub(crate) reasons: HashMap<String, String>,
}

/// Partition the corpus for `q` and rank both halves.
///
/// The ordering corpus is built over `docs` entire — see the module docs — and
/// the filter runs after it.
pub(crate) fn select<'a>(
    pcfg: &'a ProjectConfig,
    docs: &'a [Doc],
    main: &'a [Doc],
    q: &PickQuery,
) -> Selection<'a> {
    let corpus = order::Corpus::with_truth(pcfg, docs, main);
    let mut claimed = Vec::new();
    let mut pickable: Vec<&Doc> = Vec::new();
    let mut blocked: Vec<&Doc> = Vec::new();
    let mut orphaned: Vec<&Doc> = Vec::new();
    let mut reasons: HashMap<String, String> = HashMap::new();
    for doc in docs {
        let Some(id) = doc.id() else { continue };
        if !candidate(pcfg, doc, q) {
            continue;
        }
        if pcfg
            .type_name_for_id(id)
            .and_then(|n| pcfg.types[n].claim_field.as_deref())
            .is_some_and(|field| doc.frontmatter.contains_key(field))
        {
            claimed.push(doc);
            continue;
        }
        if let Some(reason) = orphan_reason(pcfg, doc, corpus.blockers()) {
            reasons.insert(id.to_string(), reason);
            orphaned.push(doc);
            continue;
        }
        if refs::live_blockers(doc, corpus.blockers()).is_empty() {
            pickable.push(doc);
        } else {
            blocked.push(doc);
        }
    }
    let pickable = corpus.ranked(&pickable);
    let orphaned = corpus.ranked(&orphaned);
    let blocked = corpus.ranked(&blocked);
    let claimed = corpus.ranked(&claimed);
    Selection {
        corpus,
        pickable,
        blocked,
        claimed,
        orphaned,
        reasons,
    }
}

/// Whether `doc` is in scope for `q` at all: right status, and right type when
/// one was named.
fn candidate(pcfg: &ProjectConfig, doc: &Doc, q: &PickQuery) -> bool {
    (doc.status() == Some(q.status.as_str())
        // A role override still routes malformed documents with no status.
        // Queue represents that repair group by the empty status name.
        || (q.role.is_some() && q.status.is_empty() && doc.status().is_none()))
        && q.role
            .as_deref()
            .is_none_or(|r| roles::waits_on(pcfg, doc, r))
        && q.type_filter
            .as_ref()
            .is_none_or(|t| type_of(pcfg, doc).as_deref() == Some(t.as_str()))
}

pub(crate) fn type_of(pcfg: &ProjectConfig, doc: &Doc) -> Option<String> {
    doc.id()
        .and_then(|id| pcfg.type_name_for_id(id))
        .map(str::to_string)
}

/// Why `doc` is orphaned, or `None` when it is not.
///
/// A type's `requires_link = { to, min }` is a **count** rule (the same one
/// [`crate::rules`] enforces at write time), so this asks the count question
/// too: a document is orphaned when fewer than `min` of its links to that type
/// still [hold](refs::Corpus::holds) — resolve to a live document whose status
/// its own type does not declare in `releases_blocks`. Reading it as "*every*
/// target still holds" would strand a task that names one shipped feature
/// alongside a planned one, which is ordinary and healthy.
///
/// Note what `releases_blocks` means here: it is where a type stops wanting
/// work, which for the shipped `feature` type includes `implemented` as well as
/// `wontfix`. A bug filed against a shipped feature is therefore *reported*
/// rather than picked — reported, with the reason, so the reader can re-link it
/// or reopen the parent, which is the whole point of not dropping it silently.
pub fn orphan_reason(
    pcfg: &ProjectConfig,
    doc: &Doc,
    blockers: &refs::Corpus<'_>,
) -> Option<String> {
    let id = doc.id()?;
    let t = pcfg.types.get(pcfg.type_name_for_id(id)?)?;
    let lr = t.requires_link.as_ref()?;
    let target = pcfg.types.get(&lr.to)?;
    let named = refs::ids_with_prefix(&doc.frontmatter, &target.prefix);
    let mut held = 0usize;
    let mut dead: Vec<String> = Vec::new();
    for tid in &named {
        if blockers.holds(tid) {
            held += 1;
        } else if let Some(status) = blockers.status_of(tid) {
            dead.push(format!(
                "{tid} is at {status}, which releases what it blocks"
            ));
        } else {
            dead.push(format!("{tid} resolves to no document"));
        }
    }
    if held >= lr.min {
        return None;
    }
    if dead.is_empty() && named.is_empty() {
        dead.push(format!("it references no {} document", lr.to));
    }
    let tail = format!("{held}/{} required {} links still hold", lr.min, lr.to);
    Some(if dead.is_empty() {
        tail
    } else {
        format!("{}; {tail}", dead.join("; "))
    })
}

/// The pickable documents at `q.status`, best first, plus the orphans.
///
/// Resolves declared work copies once. `now` is the instant observations are
/// measured against; projects without declarations never open git.
pub fn pick(prj: &Project, docs: &[Doc], q: &PickQuery, now: OffsetDateTime) -> PickResult {
    let view = work::effective(prj, docs, now);
    pick_effective(prj, &view, q, now)
}

pub fn pick_effective(
    prj: &Project,
    view: &work::Effective<'_>,
    q: &PickQuery,
    now: OffsetDateTime,
) -> PickResult {
    let main = view.main.as_ref();
    let docs = view.docs.as_ref();
    let sel = select(&prj.pcfg, docs, main, q);
    let total = sel.pickable.len();
    let items = sel
        .pickable
        .iter()
        .take(q.limit)
        .enumerate()
        .map(|(i, doc)| {
            let mut item = item(prj, &sel.corpus, doc, i + 1, now);
            item.reference = view.reference(doc);
            item
        })
        .collect();
    let orphaned = sel
        .orphaned
        .iter()
        .map(|doc| {
            let id = doc.id().unwrap_or_default().to_string();
            Orphan {
                reason: sel.reasons.get(&id).cloned().unwrap_or_default(),
                id,
                type_name: type_of(&prj.pcfg, doc),
                title: doc.title.clone(),
                status: doc.status().unwrap_or_default().to_string(),
                path: rel_path(prj, doc),
                reference: view.reference(doc),
            }
        })
        .collect();
    PickResult {
        schema: SCHEMA,
        status: q.status.clone(),
        type_filter: q.type_filter.clone(),
        role: q.role.clone(),
        limit: q.limit,
        total,
        items,
        orphaned,
        explain: None,
        notices: view.notices.clone(),
        unavailable: main
            .iter()
            .zip(docs)
            .filter_map(|(doc, copy)| {
                let id = doc.id()?;
                let occupied = view.refs.contains_key(id) && candidate(&prj.pcfg, doc, q);
                let claimed =
                    candidate(&prj.pcfg, copy, q) && work::claim(&prj.pcfg, copy, now).is_some();
                (occupied || claimed)
                    .then(|| explain_effective(prj, view, q, id, now).ok())
                    .flatten()
                    .filter(|e| !e.listed)
            })
            .collect(),
    }
}

pub(crate) fn item(
    prj: &Project,
    corpus: &order::Corpus<'_>,
    doc: &Doc,
    position: usize,
    now: OffsetDateTime,
) -> Item {
    let type_name = type_of(&prj.pcfg, doc);
    // The same two predicates `observe::observations` runs, in the same
    // overflow-before-stuck order — composed here rather than called through it,
    // because an item carries only its own observations while `observations`
    // sorts the whole corpus. A deliberate duplication: change the set or the
    // order there and this has to follow.
    let mut observations = Vec::new();
    if let Some(t) = type_name.as_deref().and_then(|n| prj.pcfg.types.get(n)) {
        observations.extend(observe::overflow(doc, t));
        observations.extend(observe::stuck(doc, t, now));
    }
    Item {
        id: doc.id().unwrap_or_default().to_string(),
        type_name,
        title: doc.title.clone(),
        status: doc.status().unwrap_or_default().to_string(),
        path: rel_path(prj, doc),
        tags: doc.frontmatter.tags().unwrap_or_default(),
        priority: order::own_priority(doc),
        effective_priority: order::effective_priority(doc, corpus),
        blocks: corpus.live_blocks_count(doc),
        position,
        observations,
        owners: roles::effective_owners(&prj.pcfg, doc),
        reference: None,
        claim: work::claim(&prj.pcfg, doc, now),
    }
}

/// The document's path relative to the project root, with `/` separators.
///
/// Absolute paths would make the payload machine-dependent — unpinnable in a
/// test, and a leak of the operator's directory layout through the node.
pub(crate) fn rel_path(prj: &Project, doc: &Doc) -> String {
    doc.path
        .strip_prefix(&prj.root)
        .unwrap_or(&doc.path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Why the document `id` is, or is not, among what [`pick`] would list.
///
/// The same [`select`] pass answers both, so a document can never be excluded
/// for one reason and explained with another. An id no document answers to is a
/// [`OpysError::NotFound`] (exit 2), the way `show` treats one.
pub fn explain(
    prj: &Project,
    docs: &[Doc],
    q: &PickQuery,
    id: &str,
    now: OffsetDateTime,
) -> Result<Explanation> {
    let view = work::effective(prj, docs, now);
    explain_effective(prj, &view, q, id, now)
}

pub fn explain_effective(
    prj: &Project,
    view: &work::Effective<'_>,
    q: &PickQuery,
    id: &str,
    now: OffsetDateTime,
) -> Result<Explanation> {
    let main = view.main.as_ref();
    let docs = view.docs.as_ref();
    view.ensure_main()?;
    let doc = prj.find(docs, id)?;
    let pcfg = &prj.pcfg;
    // The precedence, in order. Type and status first: neither is a judgement
    // about the document, only about the question that was asked.
    if let Some(want) = &q.type_filter {
        let have = type_of(pcfg, doc);
        if have.as_deref() != Some(want.as_str()) {
            let named = have.clone().unwrap_or_else(|| "unknown".to_string());
            return Ok(Explanation {
                id: id.to_string(),
                listed: false,
                message: format!("{id}: not listed — its type is {named}, not {want}"),
                why: Why::OtherType { type_name: have },
            });
        }
    }
    let status = doc.status().unwrap_or_default().to_string();
    if status != q.status {
        if let Some(reference) = view.refs.get(id).filter(|_| {
            main.iter()
                .any(|d| d.id() == Some(id) && d.status() == Some(q.status.as_str()))
        }) {
            return Ok(Explanation {
                id: id.to_string(),
                listed: false,
                message: format!("{id}: not listed — occupied by {reference}"),
                why: Why::Occupied {
                    reference: reference.clone(),
                },
            });
        }
        return Ok(Explanation {
            id: id.to_string(),
            listed: false,
            message: format!("{id}: not listed — it is at {status}, not {}", q.status),
            why: Why::OtherStatus { status },
        });
    }
    if let Some(role) = &q.role {
        if !roles::waits_on(pcfg, doc, role) {
            let owners = roles::effective_owners(pcfg, doc);
            let named = if owners.is_empty() {
                "nobody".to_string()
            } else {
                owners.join(", ")
            };
            return Ok(Explanation {
                id: id.to_string(),
                listed: false,
                message: format!("{id}: not listed — waits on {named}, not {role}"),
                why: Why::OtherRole { owners },
            });
        }
    }
    if let Some(claim) = work::claim(pcfg, doc, now) {
        return Ok(Explanation {
            id: id.to_string(),
            listed: false,
            message: format!("{id}: not listed — claimed by {}", claim.holder),
            why: Why::Claimed {
                claim,
                reference: view.reference(doc),
            },
        });
    }
    let sel = select(pcfg, docs, main, q);
    if let Some(detail) = sel.reasons.get(id) {
        return Ok(Explanation {
            id: id.to_string(),
            listed: false,
            message: format!("{id}: not listed — orphaned: {detail}"),
            why: Why::Orphaned {
                detail: detail.clone(),
            },
        });
    }
    let blockers: Vec<String> = refs::live_blockers(doc, sel.corpus.blockers())
        .into_iter()
        .map(|(bid, _)| bid)
        .collect();
    if !blockers.is_empty() {
        return Ok(Explanation {
            id: id.to_string(),
            listed: false,
            message: format!("{id}: not listed — blocked by {}", blockers.join(", ")),
            why: Why::Blocked { blockers },
        });
    }
    let total = sel.pickable.len();
    let position = sel
        .pickable
        .iter()
        .position(|d| d.id() == Some(id))
        .map_or(0, |i| i + 1);
    if position > q.limit {
        return Ok(Explanation {
            id: id.to_string(),
            listed: false,
            message: format!(
                "{id}: not listed — pickable but ordered {position} of {total}, behind the {} listed (limit {})",
                q.limit.min(total),
                q.limit
            ),
            why: Why::Behind { position, total },
        });
    }
    let item = item(prj, &sel.corpus, doc, position, now);
    Ok(Explanation {
        id: id.to_string(),
        listed: true,
        message: listed_line(&item, total),
        why: Why::Listed { position, total },
    })
}

/// The one-line "why this was listed" a `--explain` run prints per item.
///
/// Byte-stable and deliberately dense: it is what a dispatcher logs when it
/// wants a record of why it picked what it picked.
pub fn listed_line(item: &Item, total: usize) -> String {
    let priority = match (item.priority, item.effective_priority) {
        (Some(own), Some(eff)) if own != eff => {
            format!("effective priority {eff} (its own {own}, inherited)")
        }
        (None, Some(eff)) => format!("effective priority {eff} (inherited)"),
        (Some(own), _) => format!("priority {own}"),
        (None, None) => "no priority".to_string(),
    };
    format!(
        "{}: listed {} of {total} — {priority}, blocks {} live document{}",
        item.id,
        item.position,
        item.blocks,
        if item.blocks == 1 { "" } else { "s" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::OpysError;
    use std::path::PathBuf;

    /// A `feature` that releases what it blocks at `implemented` *and* at
    /// `wontfix`, a `task` that must link one and declares the `int priority`
    /// field, and a `note` that requires nothing.
    fn project() -> Project {
        let pcfg: ProjectConfig = toml::from_str(
            r#"
[types.feature]
prefix = "FEAT"
statuses = ["planned", "implemented", "wontfix"]
default_status = "planned"
releases_blocks = ["implemented", "wontfix"]

[types.task]
prefix = "TASK"
statuses = ["todo", "doing", "done"]
default_status = "todo"
terminal_statuses = ["done"]
requires_link = { to = "feature", min = 1 }
[types.task.ttl]
doing = { after = "10m", then = "stuck" }
[types.task.fields.priority]
type = "int"

[types.note]
prefix = "NOTE"
statuses = ["open"]
default_status = "open"
"#,
        )
        .expect("parses");
        Project {
            root: PathBuf::from("/p"),
            base: PathBuf::from("/p/opys"),
            pcfg,
        }
    }

    fn doc(id: &str, status: &str, extra: &str) -> Doc {
        Doc::parse(
            PathBuf::from(format!("/p/opys/{id}.md")),
            &format!("---\nid: {id}\nstatus: {status}\n{extra}---\n\n# Title of {id}\n"),
        )
        .expect("parses")
    }

    fn q(status: &str) -> PickQuery {
        PickQuery {
            status: status.to_string(),
            type_filter: None,
            role: None,
            limit: DEFAULT_LIMIT,
        }
    }

    fn now() -> OffsetDateTime {
        crate::history::parse("2026-01-02T03:04:05Z").expect("parses")
    }

    fn ids(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.id.as_str()).collect()
    }

    /// A live blocker excludes a document silently — it is not a problem, and it
    /// clears on its own — and the same document is listed once the blocker
    /// reaches a status its type releases at.
    #[test]
    fn a_blocked_document_is_excluded_until_its_blocker_releases() {
        let prj = project();
        let held = vec![
            doc("FEAT-0001", "planned", "blocks:\n  TASK-0002: Two\n"),
            doc(
                "TASK-0002",
                "todo",
                "blocked_by:\n  FEAT-0001: One\nreferences:\n  FEAT-0001: One\n",
            ),
        ];
        let out = pick(&prj, &held, &q("todo"), now());
        assert!(out.items.is_empty());
        assert!(out.orphaned.is_empty());
        assert_eq!(out.total, 0);

        let released = vec![
            doc("FEAT-0001", "implemented", "blocks:\n  TASK-0002: Two\n"),
            doc(
                "TASK-0002",
                "todo",
                "blocked_by:\n  FEAT-0001: One\nreferences:\n  FEAT-0001: One\n",
            ),
        ];
        let out = pick(&prj, &released, &q("todo"), now());
        // …and the feature that released is now also what orphans it: a
        // released parent is exactly what `releases_blocks` declares.
        assert!(out.items.is_empty());
        assert_eq!(ids_of_orphans(&out), vec!["TASK-0002"]);
    }

    fn ids_of_orphans(out: &PickResult) -> Vec<&str> {
        out.orphaned.iter().map(|o| o.id.as_str()).collect()
    }

    /// The plain wontfix case FEAT-0057 names: a task whose only feature is
    /// `wontfix` is reported with the reason, not picked and not dropped.
    #[test]
    fn a_task_under_a_wontfix_feature_is_orphaned() {
        let prj = project();
        let docs = vec![
            doc("FEAT-0001", "wontfix", ""),
            doc("TASK-0002", "todo", "references:\n  FEAT-0001: One\n"),
        ];
        let out = pick(&prj, &docs, &q("todo"), now());
        assert!(out.items.is_empty());
        assert_eq!(
            out.orphaned[0].reason,
            "FEAT-0001 is at wontfix, which releases what it blocks; \
             0/1 required feature links still hold"
        );
    }

    /// `requires_link` is a **count**: one live feature satisfies `min = 1`
    /// however many released ones are named beside it.
    #[test]
    fn one_live_target_satisfies_a_min_of_one() {
        let prj = project();
        let docs = vec![
            doc("FEAT-0001", "implemented", ""),
            doc("FEAT-0002", "planned", ""),
            doc(
                "TASK-0003",
                "todo",
                "references:\n  FEAT-0001: One\n  FEAT-0002: Two\n",
            ),
        ];
        let out = pick(&prj, &docs, &q("todo"), now());
        assert_eq!(ids(&out.items), vec!["TASK-0003"]);
        assert!(out.orphaned.is_empty());
    }

    /// The two ways a target can fail are different facts, and the reason says
    /// which: released at a status its type declares, or nothing there at all.
    #[test]
    fn the_reason_tells_a_released_target_from_a_missing_one() {
        let prj = project();
        let docs = vec![doc("TASK-0003", "todo", "references:\n  FEAT-0009: Gone\n")];
        let out = pick(&prj, &docs, &q("todo"), now());
        assert_eq!(
            out.orphaned[0].reason,
            "FEAT-0009 resolves to no document; 0/1 required feature links still hold"
        );

        let docs = vec![doc("TASK-0004", "todo", "")];
        let out = pick(&prj, &docs, &q("todo"), now());
        assert_eq!(
            out.orphaned[0].reason,
            "it references no feature document; 0/1 required feature links still hold"
        );
    }

    /// Orphaned wins over blocked: a document that is both is reported once, as
    /// the condition that will not clear by itself.
    #[test]
    fn orphaned_is_reported_ahead_of_blocked() {
        let prj = project();
        let docs = vec![
            doc("FEAT-0001", "wontfix", ""),
            doc("NOTE-0002", "open", "blocks:\n  TASK-0003: Three\n"),
            doc(
                "TASK-0003",
                "todo",
                "blocked_by:\n  NOTE-0002: Two\nreferences:\n  FEAT-0001: One\n",
            ),
        ];
        let out = pick(&prj, &docs, &q("todo"), now());
        assert!(out.items.is_empty());
        assert_eq!(ids_of_orphans(&out), vec!["TASK-0003"]);
        let why = explain(&prj, &docs, &q("todo"), "TASK-0003", now()).expect("found");
        assert!(matches!(why.why, Why::Orphaned { .. }), "{why:?}");
    }

    /// A type with no `requires_link` is never orphaned, whatever it references.
    #[test]
    fn a_type_without_requires_link_is_never_orphaned() {
        let prj = project();
        let docs = vec![doc("NOTE-0001", "open", "references:\n  FEAT-0009: Gone\n")];
        let out = pick(&prj, &docs, &q("open"), now());
        assert_eq!(ids(&out.items), vec!["NOTE-0001"]);
        assert!(out.orphaned.is_empty());
    }

    /// The ordering is `order`'s, and the limit truncates `items` while `total`
    /// keeps counting — that number is what `--explain` reports "behind" against.
    #[test]
    fn the_order_is_the_ordering_modules_and_the_limit_only_truncates() {
        let prj = project();
        // Both halves of the blocker relation, as `links::reconcile_blockers`
        // writes them: the urgent task is held up by the one that blocks it, so
        // it is excluded from the pick — and still lifts its blocker to its `1`.
        let docs = vec![
            doc("FEAT-0001", "planned", ""),
            doc(
                "TASK-0002",
                "todo",
                "priority: 30\nblocks:\n  TASK-0004: Urgent\nreferences:\n  FEAT-0001: One\n",
            ),
            doc(
                "TASK-0003",
                "todo",
                "priority: 20\nreferences:\n  FEAT-0001: One\n",
            ),
            doc(
                "TASK-0004",
                "todo",
                "priority: 1\nblocked_by:\n  TASK-0002: Two\nreferences:\n  FEAT-0001: One\n",
            ),
        ];
        let out = pick(&prj, &docs, &q("todo"), now());
        assert_eq!(ids(&out.items), vec!["TASK-0002", "TASK-0003"]);
        assert_eq!(out.items[0].effective_priority, Some(1));
        assert_eq!(out.items[0].priority, Some(30));
        assert_eq!(out.items[0].blocks, 1);
        assert_eq!(out.items[0].position, 1);
        assert_eq!(out.items[0].path, "opys/TASK-0002.md");

        // The same order the whole ranking gives, which is `list --sort`'s,
        // minus the one a live blocker keeps out of the pick.
        let corpus = order::Corpus::of_docs(&prj.pcfg, &docs);
        let wanted: Vec<&str> = corpus
            .ranked(&docs.iter().skip(1).collect::<Vec<_>>())
            .iter()
            .filter_map(|d| d.id())
            .filter(|id| *id != "TASK-0004")
            .collect();
        assert_eq!(ids(&out.items), wanted);

        let mut one = q("todo");
        one.limit = 1;
        let out = pick(&prj, &docs, &one, now());
        assert_eq!(ids(&out.items), vec!["TASK-0002"]);
        assert_eq!(out.total, 2);
    }

    /// The type filter narrows the candidates without narrowing the corpus the
    /// ordering is computed over.
    #[test]
    fn the_type_filter_narrows_the_candidates_only() {
        let prj = project();
        let docs = vec![
            doc("FEAT-0001", "planned", ""),
            doc("NOTE-0002", "open", ""),
            doc(
                "TASK-0003",
                "todo",
                "priority: 5\nreferences:\n  FEAT-0001: One\n",
            ),
        ];
        let mut only_tasks = q("todo");
        only_tasks.type_filter = Some("task".to_string());
        assert_eq!(
            ids(&pick(&prj, &docs, &only_tasks, now()).items),
            vec!["TASK-0003"]
        );
        let why = explain(&prj, &docs, &only_tasks, "NOTE-0002", now()).expect("found");
        assert_eq!(
            why.message,
            "NOTE-0002: not listed — its type is note, not task"
        );
    }

    /// An item carries its type's observations, overflow before stuck.
    #[test]
    fn an_item_carries_its_types_observations() {
        let prj = project();
        let docs = vec![
            doc("FEAT-0001", "planned", ""),
            doc(
                "TASK-0002",
                "doing",
                "history:\n  \"2026-01-02T03:04:05.000000Z\": doing\nreferences:\n  FEAT-0001: One\n",
            ),
        ];
        let late = crate::history::parse("2026-01-02T03:20:00Z").expect("parses");
        let out = pick(&prj, &docs, &q("doing"), late);
        assert_eq!(out.items[0].observations.len(), 1);
        assert_eq!(
            out.items[0].observations[0].to_string(),
            "TASK-0002: stuck in doing since 2026-01-02T03:04:05.000000Z (ttl 10m) \
             — `--fix` moves it to stuck"
        );
        // …and nothing is stuck at the instant it entered the status.
        let out = pick(&prj, &docs, &q("doing"), now());
        assert!(out.items[0].observations.is_empty());
    }

    /// Every `--explain` reason, over one corpus, in the precedence they are
    /// checked in.
    #[test]
    fn explain_names_a_reason_for_each_way_a_document_is_left_out() {
        let prj = project();
        let docs = vec![
            doc("FEAT-0001", "planned", ""),
            doc("FEAT-0002", "wontfix", ""),
            doc(
                "TASK-0003",
                "todo",
                "priority: 1\nblocks:\n  TASK-0007: Seven\nreferences:\n  FEAT-0001: One\n",
            ),
            doc(
                "TASK-0004",
                "todo",
                "priority: 2\nreferences:\n  FEAT-0001: One\n",
            ),
            doc("TASK-0005", "doing", "references:\n  FEAT-0001: One\n"),
            doc("TASK-0006", "todo", "references:\n  FEAT-0002: Two\n"),
            doc(
                "TASK-0007",
                "todo",
                "blocked_by:\n  TASK-0003: Three\nreferences:\n  FEAT-0001: One\n",
            ),
        ];
        let mut one = q("todo");
        one.limit = 1;
        let msg = |id: &str| {
            explain(&prj, &docs, &one, id, now())
                .expect("found")
                .message
        };
        assert_eq!(
            msg("TASK-0003"),
            "TASK-0003: listed 1 of 2 — priority 1, blocks 1 live document"
        );
        assert_eq!(
            msg("TASK-0004"),
            "TASK-0004: not listed — pickable but ordered 2 of 2, behind the 1 listed (limit 1)"
        );
        assert_eq!(
            msg("TASK-0005"),
            "TASK-0005: not listed — it is at doing, not todo"
        );
        assert_eq!(
            msg("TASK-0006"),
            "TASK-0006: not listed — orphaned: FEAT-0002 is at wontfix, which releases what it blocks; \
             0/1 required feature links still hold"
        );
        assert_eq!(
            msg("TASK-0007"),
            "TASK-0007: not listed — blocked by TASK-0003"
        );
        assert!(matches!(
            explain(&prj, &docs, &one, "TASK-0404", now()),
            Err(OpysError::NotFound { .. })
        ));
    }

    /// The inherited case spells both numbers, because "why is this first" is
    /// unanswerable from the effective number alone.
    #[test]
    fn the_listed_line_names_an_inherited_priority_as_such() {
        let prj = project();
        let docs = vec![
            doc("FEAT-0001", "planned", ""),
            doc(
                "TASK-0002",
                "todo",
                "priority: 30\nblocks:\n  TASK-0003: Three\nreferences:\n  FEAT-0001: One\n",
            ),
            doc(
                "TASK-0003",
                "todo",
                "priority: 1\nblocked_by:\n  TASK-0002: Two\nreferences:\n  FEAT-0001: One\n",
            ),
        ];
        // The `1` is inherited from a document the pick itself excludes, which
        // is the ordinary case: the reason TASK-0003 is out is TASK-0002.
        let why = explain(&prj, &docs, &q("todo"), "TASK-0002", now()).expect("found");
        assert_eq!(
            why.message,
            "TASK-0002: listed 1 of 1 — effective priority 1 (its own 30, inherited), \
             blocks 1 live document"
        );
        assert!(why.listed);
    }

    /// An empty answer is an answer: no items, no orphans, and nothing that
    /// looks like a failure.
    #[test]
    fn a_status_with_nothing_at_it_is_an_empty_result() {
        let prj = project();
        let docs = vec![doc("FEAT-0001", "planned", "")];
        let out = pick(&prj, &docs, &q("todo"), now());
        assert_eq!(out.total, 0);
        assert!(out.items.is_empty());
        assert!(out.orphaned.is_empty());
        assert_eq!(out.schema, SCHEMA);
    }

    /// A misspelled status or type is refused rather than answered with nothing:
    /// the caller could not otherwise tell a typo from an empty queue.
    #[test]
    fn a_status_or_type_no_type_declares_is_refused() {
        let prj = project();
        assert!(q("todo").validate(&prj.pcfg).is_ok());
        assert_eq!(
            q("to-do").validate(&prj.pcfg).unwrap_err(),
            "unknown status \"to-do\" (no configured type declares it; known: \
             doing, done, implemented, open, planned, todo, wontfix)"
        );
        let mut bad_type = q("todo");
        bad_type.type_filter = Some("tasks".to_string());
        assert_eq!(
            bad_type.validate(&prj.pcfg).unwrap_err(),
            "unknown type \"tasks\" (configured: feature, note, task)"
        );
        // A status a *different* type declares is still wrong for this one.
        let mut wrong_pair = q("open");
        wrong_pair.type_filter = Some("task".to_string());
        assert_eq!(
            wrong_pair.validate(&prj.pcfg).unwrap_err(),
            "unknown status \"open\" for type 'task' (allowed: todo, doing, done)"
        );
    }
}
