//! The node's access to declared work copies. Reads use the engine's projection;
//! writes require the actual checkout and never turn a read projection into files.

use opys_engine::{doc::Doc, git, observe, project::Project, work};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct WorkCopy {
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claim: Option<work::Claim>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only_reason: Option<String>,
    /// Filled by the API only for a checkout this node actually serves.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit_cid: Option<String>,
    #[serde(skip)]
    pub(crate) unavailable: bool,
}

pub(crate) fn access(prj: &Project, view: &work::Effective<'_>, id: &str) -> Option<WorkCopy> {
    let unknown = view.unknown.get(id);
    let reference = view
        .refs
        .get(id)
        .or_else(|| unknown.map(|u| &u.reference))?;
    let reason = if let Some(unknown) = unknown {
        Some(format!(
            "Work copy on {reference} is unavailable: {}",
            unknown.detail
        ))
    } else {
        (!git::work_checkout_matches(prj, reference).unwrap_or(false))
            .then(|| format!("This document is on {reference}. Editing requires its served checkout; the main copy cannot be edited while this branch is open."))
    };
    Some(WorkCopy {
        reference: reference.clone(),
        claim: view
            .docs
            .iter()
            .find(|d| d.id() == Some(id))
            .and_then(|d| work::claim(&prj.pcfg, d, observe::now())),
        read_only_reason: reason,
        edit_cid: None,
        unavailable: unknown.is_some(),
    })
}

/// The stages of this copy, interpreted from its engine-resolved ref.
pub(crate) fn stages<'a>(prj: &'a Project, id: &str, reference: &str) -> Option<&'a [String]> {
    prj.pcfg.type_name_for_id(id).and_then(|name| {
        prj.pcfg.types[name]
            .work_branches
            .values()
            .find(|branch| {
                let wanted = branch.reference.replace("{id}", id);
                reference == wanted || reference.ends_with(&format!("/{wanted}"))
            })
            .map(|branch| branch.stages.as_slice())
    })
}

pub(crate) fn guard(
    prj: &Project,
    docs: &[Doc],
    action: &crate::action::Action,
) -> opys_engine::error::Result<()> {
    use crate::action::Action;
    use opys_engine::error::usage;
    if !work::declared(&prj.pcfg) {
        return Ok(());
    }
    let ids: Vec<&str> = match action {
        Action::New { .. } | Action::VerifyFix { id: None } => return Ok(()),
        Action::VerifyFix { id: Some(id) }
        | Action::SetStatus { id, .. }
        | Action::Tag { id, .. }
        | Action::Log { id, .. }
        | Action::Close { id, .. }
        | Action::EditBody { id, .. }
        | Action::SetField { id, .. }
        | Action::RemoveField { id, .. }
        | Action::Apply { id, .. } => vec![id],
        Action::Block { id, by } | Action::Unblock { id, by } => vec![id, by],
    };
    let view = work::effective(prj, docs, observe::now());
    view.ensure_main()?;
    for id in ids {
        let Some(copy) = access(prj, &view, id) else {
            continue;
        };
        if let Some(reason) = copy.read_only_reason {
            return Err(usage(reason));
        }
        if matches!(action, Action::Close { .. }) {
            return Err(usage(
                "Work branches cannot complete a document; merge through the orchestrator first.",
            ));
        }
        let target = match action {
            Action::SetStatus { status, .. } => Some(status.as_str()),
            Action::Apply { status, .. } => status.as_deref(),
            _ => None,
        };
        if let Some(target) = target {
            if stages(prj, id, &copy.reference).is_none_or(|s| !s.iter().any(|s| s == target)) {
                return Err(usage(format!(
                    "{target} is outside the stages of {}; merging belongs to the orchestrator",
                    copy.reference
                )));
            }
        }
    }
    Ok(())
}
