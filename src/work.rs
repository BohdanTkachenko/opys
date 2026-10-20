//! Effective copies for declared work branches (ADR-0070).
//!
//! This is a read projection, never a corpus to flush. Main remains the lookup
//! for blocker release and completion. Undeclared branches are not consulted.
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

use serde::Serialize;
use time::OffsetDateTime;

use crate::{doc::Doc, git, observe, project::Project, project_config::ProjectConfig};

#[derive(Debug, Clone, Serialize)]
pub struct Claim {
    pub holder: String,
    pub since: Option<String>,
    pub age_seconds: Option<i64>,
}

/// Claims have no separate timeline. `updated` is the last recorded activity
/// on the claimed copy; unknown timestamps stay unknown, never become zero.
pub fn claim(pcfg: &ProjectConfig, doc: &Doc, now: OffsetDateTime) -> Option<Claim> {
    let t = &pcfg.types[pcfg.type_name_for_id(doc.id()?)?];
    let value = doc.frontmatter.get(t.claim_field.as_deref()?)?;
    let holder = value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| serde_json::to_string(value).unwrap_or_default());
    let since = doc.frontmatter.get_str("updated").map(str::to_owned);
    let age_seconds = since
        .as_deref()
        .and_then(crate::history::parse)
        .map(|then| (now - then).whole_seconds().max(0));
    Some(Claim {
        holder,
        since,
        age_seconds,
    })
}

pub fn declared(pcfg: &ProjectConfig) -> bool {
    pcfg.types.values().any(|t| !t.work_branches.is_empty())
}

/// A snapshot shared by selection, display and verification. Absent declarations
/// cost no git access and no document clones.
pub struct Effective<'a> {
    pub docs: Cow<'a, [Doc]>,
    /// Release decisions always resolve against this independently established baseline.
    pub main: Cow<'a, [Doc]>,
    pub main_is_current: bool,
    pub baseline_error: Option<String>,
    pub refs: BTreeMap<String, String>,
    pub observations: Vec<observe::Observation>,
    pub notices: Vec<String>,
}

impl Effective<'_> {
    pub fn ensure_main(&self) -> crate::error::Result<()> {
        match &self.baseline_error {
            Some(error) => Err(crate::error::usage(error.clone())),
            None => Ok(()),
        }
    }

    fn into_owned(self) -> Effective<'static> {
        Effective {
            docs: Cow::Owned(self.docs.into_owned()),
            main: Cow::Owned(self.main.into_owned()),
            main_is_current: self.main_is_current,
            baseline_error: self.baseline_error,
            refs: self.refs,
            observations: self.observations,
            notices: self.notices,
        }
    }

    pub fn reference(&self, doc: &Doc) -> Option<String> {
        self.refs.get(doc.id()?).cloned()
    }
}

pub fn effective<'a>(prj: &Project, current: &'a [Doc], now: OffsetDateTime) -> Effective<'a> {
    if !declared(&prj.pcfg) {
        return from_survey(prj, current, git::Survey::default(), now);
    }
    let baseline = match git::work_main(prj) {
        Ok(baseline) => baseline,
        Err(error) => {
            let error = format!("cannot establish main baseline: {error}");
            return Effective {
                docs: Cow::Owned(Vec::new()),
                main: Cow::Owned(Vec::new()),
                main_is_current: false,
                baseline_error: Some(error.clone()),
                refs: BTreeMap::new(),
                observations: Vec::new(),
                notices: vec![error],
            };
        }
    };
    let main = baseline.as_deref().unwrap_or(current);
    let survey = git::work_survey(
        prj,
        &main
            .iter()
            .filter_map(|d| Some((d.id()?.to_string(), d.status()?.to_string())))
            .collect(),
    );
    if baseline.is_some() {
        let mut view = from_survey(prj, main, survey, now).into_owned();
        view.main_is_current = false;
        view
    } else {
        from_survey(prj, current, survey, now)
    }
}

/// Ref labels are the reader's labels: `spec/ID` locally and
/// `origin/spec/ID` remotely. A remote's branch component may match the same
/// template, but only after the reader's explicit branches allowlist admits it.
pub(crate) fn matches_ref(reference: &git::Ref, template: &str, id: &str) -> bool {
    let wanted = template.replace("{id}", id);
    reference.label == wanted
        || (reference.name.starts_with("refs/remotes/")
            && reference
                .label
                .split_once('/')
                .is_some_and(|(_, branch)| branch == wanted))
}

pub fn from_survey<'a>(
    prj: &Project,
    main: &'a [Doc],
    mut survey: git::Survey,
    now: OffsetDateTime,
) -> Effective<'a> {
    let mut out = Effective {
        docs: Cow::Borrowed(main),
        main: Cow::Borrowed(main),
        main_is_current: true,
        baseline_error: None,
        refs: BTreeMap::new(),
        observations: Vec::new(),
        notices: survey.notices,
    };
    // Stable even for a caller supplying a survey in arbitrary order. A local
    // label sorts before its remote only by spelling; no freshness is inferred.
    survey
        .refs
        .sort_by(|a, b| a.reference.label.cmp(&b.reference.label));
    for state in &survey.refs {
        if let Err(error) = &state.changed {
            out.notices
                .push(format!("git: {}: {error}", state.reference.label));
        }
    }
    for (index, doc) in main.iter().enumerate() {
        let Some(id) = doc.id() else { continue };
        let Some(name) = prj.pcfg.type_name_for_id(id) else {
            continue;
        };
        let t = &prj.pcfg.types[name];
        for (kind, branch) in &t.work_branches {
            let mut copies: Vec<_> = survey
                .refs
                .iter()
                .filter(|s| matches_ref(&s.reference, &branch.reference, id))
                .filter_map(|s| {
                    let changed = s.changed.as_ref().ok()?;
                    if changed.deleted.iter().any(|deleted| deleted == id) {
                        return None;
                    }
                    Some((
                        &s.reference,
                        changed.docs.iter().find(|d| d.id() == Some(id))?,
                    ))
                })
                .collect();
            // Match identities before deduplicating commits. Among matching
            // aliases prefer the first fully qualified name (local before
            // remote); different commits keep the documented label order.
            copies.sort_by(|(a, _), (b, _)| a.name.cmp(&b.name));
            let mut seen = BTreeSet::new();
            copies.retain(|(r, _)| seen.insert(r.commit.clone()));
            copies.sort_by(|(a, _), (b, _)| a.label.cmp(&b.label));
            if copies.len() > 1 {
                out.observations.push(observation(
                    id,
                    format!(
                        "two or more open work branches of kind {kind}: {}",
                        copies
                            .iter()
                            .map(|(r, _)| r.label.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
            for (reference, copy) in copies {
                out.observations.extend(claim_observation(
                    &prj.pcfg,
                    copy,
                    Some(&reference.label),
                    now,
                ));
                if doc.status() != Some(branch.from.as_str()) {
                    out.observations.push(observation(
                        id,
                        format!(
                            "main is at {}, expected {} while {} is open",
                            doc.status().unwrap_or("<missing>"),
                            branch.from,
                            reference.label
                        ),
                    ));
                }
                if !branch
                    .stages
                    .iter()
                    .any(|s| copy.status() == Some(s.as_str()))
                {
                    out.observations.push(observation(
                        id,
                        format!(
                            "{} is at {}, not a declared stage of {kind}",
                            reference.label,
                            copy.status().unwrap_or("<missing>")
                        ),
                    ));
                    continue;
                }
                // A terminal stage cannot complete anything off main, even if
                // it was accidentally included in a declaration.
                if t.terminal_statuses
                    .iter()
                    .any(|s| copy.status() == Some(s.as_str()))
                {
                    continue;
                }
                if !out.refs.contains_key(id) {
                    out.refs.insert(id.to_string(), reference.label.clone());
                    out.docs.to_mut()[index] = copy.clone();
                }
            }
        }
        if !out.refs.contains_key(id) {
            out.observations
                .extend(claim_observation(&prj.pcfg, doc, None, now));
        }
    }
    out
}

fn claim_observation(
    pcfg: &ProjectConfig,
    doc: &Doc,
    reference: Option<&str>,
    now: OffsetDateTime,
) -> Option<observe::Observation> {
    let id = doc.id()?;
    let ttl = pcfg.types[pcfg.type_name_for_id(id)?]
        .claim_ttl
        .as_deref()?;
    let claim = claim(pcfg, doc, now)?;
    let since = claim.since.as_deref()?;
    if now - crate::history::parse(since)? <= observe::parse_duration(ttl)? {
        return None;
    }
    let reference = reference.map(|r| format!(" on {r}")).unwrap_or_default();
    Some(observation(
        id,
        format!(
            "claim by {} since {since}{reference} exceeds claim_ttl {ttl}",
            claim.holder
        ),
    ))
}

fn observation(id: &str, message: String) -> observe::Observation {
    observe::Observation {
        id: id.to_string(),
        kind: observe::Kind::WorkBranch { message },
        target: None,
    }
}

#[cfg(test)]
mod tests;
