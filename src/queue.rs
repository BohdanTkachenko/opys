//! Role queues derived from the pick oracle and verification, never stored.
//! Callers supply findings: the CLI computes them fresh, while the node uses
//! the corpus actor's cached verification snapshot.

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;
use time::OffsetDateTime;

use crate::{
    diagnostics::Finding, doc::Doc, history, next, observe, project::Project,
    project_config::ProjectConfig, refs, roles,
};

pub const SCHEMA: &str = "opys.queue/1";

#[derive(Debug, Clone)]
pub struct QueueQuery {
    pub role: String,
    pub type_filter: Option<String>,
    pub limit: usize,
}

impl QueueQuery {
    pub fn validate(&self, pcfg: &ProjectConfig) -> Result<(), String> {
        roles::validate_role(pcfg, &self.role)?;
        if let Some(name) = &self.type_filter {
            if !pcfg.types.contains_key(name) {
                return Err(format!(
                    "unknown type {name:?} (configured: {})",
                    pcfg.types.names().collect::<Vec<_>>().join(", ")
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct QueueResult {
    pub schema: &'static str,
    pub role: String,
    #[serde(rename = "type")]
    pub type_filter: Option<String>,
    pub limit: usize,
    pub total: usize,
    pub groups: Vec<Group>,
    pub attention: Vec<Attention>,
    pub project_problems: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Group {
    #[serde(rename = "type")]
    pub type_name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub owners: Vec<String>,
    pub owned: bool,
    /// Pickable documents before the limit, as in next.
    pub total: usize,
    pub items: Vec<QueueItem>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub in_progress: Vec<QueueItem>,
    pub blocked: Vec<Blocked>,
    pub orphaned: Vec<next::Orphan>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueueItem {
    #[serde(flatten)]
    pub item: next::Item,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub owner_override: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Blocked {
    pub id: String,
    pub title: String,
    pub path: String,
    pub blocked_by: Vec<String>,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Attention {
    pub id: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    pub title: String,
    pub status: String,
    pub path: String,
    pub reasons: Vec<Reason>,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claim: Option<crate::work::Claim>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Reason {
    pub kind: &'static str,
    pub message: String,
}

/// One group uses next's complete partition and item builder, so role filters,
/// orphan precedence, ranking and item fields cannot diverge from the oracle.
pub fn build(
    prj: &Project,
    docs: &[Doc],
    q: &QueueQuery,
    now: OffsetDateTime,
    findings: &[Finding],
) -> QueueResult {
    let view = crate::work::effective(prj, docs, now);
    build_effective(prj, &view, q, now, findings)
}

pub fn build_effective(
    prj: &Project,
    view: &crate::work::Effective<'_>,
    q: &QueueQuery,
    now: OffsetDateTime,
    findings: &[Finding],
) -> QueueResult {
    let main = view.main.as_ref();
    let docs = view.docs.as_ref();
    let mut groups = Vec::new();
    let mut waiting = HashSet::new();
    for name in prj
        .pcfg
        .types
        .names()
        .filter(|n| q.type_filter.as_deref().is_none_or(|t| t == *n))
    {
        let t = &prj.pcfg.types[name];
        // Preserve malformed overridden documents too: verification diagnoses
        // the status; it must not hide work from the role that can repair it.
        let mut statuses = t.statuses.clone();
        for doc in docs.iter().filter(|d| {
            next::type_of(&prj.pcfg, d).as_deref() == Some(name)
                && roles::is_override(&prj.pcfg, d)
                && roles::waits_on(&prj.pcfg, d, &q.role)
        }) {
            let status = doc.status().unwrap_or_default().to_string();
            if !statuses.contains(&status) {
                statuses.push(status);
            }
        }
        for status in &statuses {
            let owners = roles::status_owners(&prj.pcfg, name, status);
            let owned = owners.contains(&q.role);
            let query = next::PickQuery {
                status: status.clone(),
                type_filter: Some(name.to_string()),
                role: Some(q.role.clone()),
                limit: q.limit,
            };
            let sel = next::select(&prj.pcfg, view, &query);
            if !owned
                && sel.pickable.is_empty()
                && sel.claimed.is_empty()
                && sel.blocked.is_empty()
                && sel.orphaned.is_empty()
            {
                continue;
            }
            waiting.extend(
                sel.pickable
                    .iter()
                    .chain(sel.claimed.iter())
                    .filter_map(|d| d.id())
                    .map(str::to_owned),
            );
            let make_item = |(i, doc): (usize, &&Doc)| {
                let mut item = next::item(prj, &sel.corpus, doc, i + 1, now);
                item.reference = view.reference(doc);
                QueueItem {
                    item,
                    since: history::entries(&doc.frontmatter)
                        .last()
                        .map(|(key, _)| key.clone())
                        .or_else(|| doc.frontmatter.get_str("created").map(str::to_owned)),
                    owner_override: roles::is_override(&prj.pcfg, doc),
                }
            };
            let items = sel
                .pickable
                .iter()
                .take(q.limit)
                .enumerate()
                .map(make_item)
                .collect();
            let in_progress = sel
                .claimed
                .iter()
                .take(q.limit)
                .enumerate()
                .map(make_item)
                .collect();
            let blocked = sel
                .blocked
                .iter()
                .map(|doc| Blocked {
                    id: doc.id().unwrap_or_default().to_string(),
                    title: doc.title.clone(),
                    path: next::rel_path(prj, doc),
                    reference: view.reference(doc),
                    blocked_by: refs::live_blockers(doc, sel.corpus.blockers())
                        .into_iter()
                        .map(|(id, _)| id)
                        .collect(),
                })
                .collect();
            let orphaned = sel
                .orphaned
                .iter()
                .map(|doc| {
                    let id = doc.id().unwrap_or_default().to_string();
                    next::Orphan {
                        reference: view.reference(doc),
                        reason: sel.reasons[&id].clone(),
                        id,
                        type_name: next::type_of(&prj.pcfg, doc),
                        title: doc.title.clone(),
                        status: doc.status().unwrap_or_default().to_string(),
                        path: next::rel_path(prj, doc),
                    }
                })
                .collect();
            groups.push(Group {
                type_name: name.to_string(),
                status: status.clone(),
                owners,
                owned,
                total: sel.pickable.len(),
                items,
                in_progress,
                blocked,
                orphaned,
            });
        }
    }
    let mut attention = Vec::new();
    let mut project_problems = Vec::new();
    if roles::escalation_role(&prj.pcfg) == Some(q.role.as_str()) {
        let corpus = refs::Corpus::of_docs(&prj.pcfg, main);
        let mut reasons: BTreeMap<String, Vec<Reason>> = BTreeMap::new();
        for doc in docs.iter().filter(|d| {
            q.type_filter
                .as_deref()
                .is_none_or(|t| next::type_of(&prj.pcfg, d).as_deref() == Some(t))
        }) {
            let Some(id) = doc.id() else { continue };
            let mut add = |kind, message| {
                reasons
                    .entry(id.to_string())
                    .or_default()
                    .push(Reason { kind, message })
            };
            if let Some(reason) = next::orphan_reason(&prj.pcfg, doc, &corpus) {
                add("orphaned", reason);
            }
            if let Some(t) = next::type_of(&prj.pcfg, doc)
                .as_deref()
                .and_then(|n| prj.pcfg.types.get(n))
            {
                for observation in observe::overflow(doc, t)
                    .into_iter()
                    .chain(observe::stuck(doc, t, now))
                {
                    let kind = match observation.kind {
                        observe::Kind::Stuck { .. } => "stuck",
                        observe::Kind::Overflow { .. } => "overflowed",
                        observe::Kind::WorkBranch { .. } => "work-branch",
                    };
                    add(kind, observation.to_string());
                }
            }
        }
        for observation in &view.observations {
            if q.type_filter
                .as_deref()
                .is_none_or(|t| prj.pcfg.type_name_for_id(&observation.id) == Some(t))
            {
                reasons
                    .entry(observation.id.clone())
                    .or_default()
                    .push(Reason {
                        kind: "work-branch",
                        message: observation.to_string(),
                    });
            }
        }
        for finding in findings {
            match &finding.doc_id {
                Some(id)
                    if q.type_filter
                        .as_deref()
                        .is_none_or(|t| prj.pcfg.type_name_for_id(id) == Some(t)) =>
                {
                    reasons.entry(id.clone()).or_default().push(Reason {
                        kind: "problem",
                        message: finding.message.clone(),
                    });
                }
                None => project_problems.push(finding.message.clone()),
                _ => {}
            }
        }
        for doc in docs {
            let Some(id) = doc.id() else { continue };
            if let Some(reasons) = reasons.remove(id) {
                waiting.insert(id.to_string());
                attention.push(Attention {
                    id: id.to_string(),
                    type_name: next::type_of(&prj.pcfg, doc),
                    title: doc.title.clone(),
                    status: doc.status().unwrap_or_default().to_string(),
                    path: next::rel_path(prj, doc),
                    reasons,
                    reference: view.reference(doc),
                    claim: crate::work::claim(&prj.pcfg, doc, now),
                });
            }
        }
        attention
            .sort_by(|a, b| (refs::id_number(&a.id), &a.id).cmp(&(refs::id_number(&b.id), &b.id)));
    }
    QueueResult {
        schema: SCHEMA,
        role: q.role.clone(),
        type_filter: q.type_filter.clone(),
        limit: q.limit,
        total: waiting.len(),
        groups,
        attention,
        project_problems,
        notices: view.notices.clone(),
    }
}
