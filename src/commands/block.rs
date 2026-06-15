//! `opys block` / `opys unblock` — record a directional blocker relationship
//! between two items (features and/or work items).
//!
//! Marking `<id>` blocked by `<by>` writes `<by>` into `<id>`'s `blocked_by`
//! map and `<id>` into `<by>`'s `blocks` map (the bidirectional link). When the
//! blocked item is a work item, its status is auto-set to `blocked` — the
//! blocker link itself serves as the `blocked_reason`. The relation spans both
//! families, so a single top-level command pair accepts any feature or
//! work-item id; the id prefix disambiguates which file to write.

use crate::commands::maybe_sync;
use crate::config::is_work_item_id;
use crate::error::{usage, OpysError, Result};
use crate::feature::Feature;
use crate::frontmatter::Frontmatter;
use crate::project::{self, Project};
use crate::refs;
use crate::work_item::WorkItem;
use crate::Ctx;

/// Mark `id` as blocked by `by`, linking both directions.
pub fn block(ctx: &Ctx, id: &str, by: &str) -> Result<()> {
    if id == by {
        return Err(usage("an item cannot block itself"));
    }
    let prj = ctx.open()?;
    let (mut feats, mut wis) = load_docs(&prj);

    let id_title =
        title_of(&feats, &wis, id).ok_or_else(|| OpysError::NotFound { id: id.to_string() })?;
    let by_title =
        title_of(&feats, &wis, by).ok_or_else(|| OpysError::NotFound { id: by.to_string() })?;

    let id_is_wi = is_work_item_id(id);
    edit_doc(&mut feats, &mut wis, id, |fm| {
        let mut changed = refs::add_to_map(fm, refs::BLOCKED_BY, by, &by_title);
        if id_is_wi && fm.status() != Some("blocked") {
            fm.set_str("status", "blocked");
            changed = true;
        }
        changed
    })?;
    edit_doc(&mut feats, &mut wis, by, |fm| {
        refs::add_to_map(fm, refs::BLOCKS, id, &id_title)
    })?;

    println!("{id} blocked by {by}");
    maybe_sync(ctx, &prj);
    Ok(())
}

/// Remove the blocker link `id` blocked-by `by` from both sides. When the
/// blocked item is a work item left at `blocked` with no remaining blockers and
/// no free-text `blocked_reason`, its status reverts to `in-progress`.
pub fn unblock(ctx: &Ctx, id: &str, by: &str) -> Result<()> {
    let prj = ctx.open()?;
    let (mut feats, mut wis) = load_docs(&prj);

    if title_of(&feats, &wis, id).is_none() {
        return Err(OpysError::NotFound { id: id.to_string() });
    }
    let linked = has_entry(&feats, &wis, id, refs::BLOCKED_BY, by)
        || has_entry(&feats, &wis, by, refs::BLOCKS, id);
    if !linked {
        return Err(usage(format!("no blocker '{by}' recorded on '{id}'")));
    }

    let id_is_wi = is_work_item_id(id);
    edit_doc(&mut feats, &mut wis, id, |fm| {
        let mut changed = refs::remove_from_map(fm, refs::BLOCKED_BY, by);
        if id_is_wi
            && fm.status() == Some("blocked")
            && refs::parse_in(fm, refs::BLOCKED_BY).is_empty()
            && fm.get_str("blocked_reason").is_none()
        {
            fm.set_str("status", "in-progress");
            changed = true;
        }
        changed
    })?;
    // The blocker may already be gone (a closed work item's struck tombstone);
    // cleaning the target side is enough, so a missing blocker is not an error.
    match edit_doc(&mut feats, &mut wis, by, |fm| {
        refs::remove_from_map(fm, refs::BLOCKS, id)
    }) {
        Ok(()) | Err(OpysError::NotFound { .. }) => {}
        Err(e) => return Err(e),
    }

    println!("{id} no longer blocked by {by}");
    maybe_sync(ctx, &prj);
    Ok(())
}

/// Load features and (when configured) live work items, ignoring parse errors —
/// a later `maybe_sync`/`verify` surfaces those.
fn load_docs(prj: &Project) -> (Vec<Feature>, Vec<WorkItem>) {
    let (feats, _) = prj.load();
    let (wis, _) = if prj.wi_cfg.is_some() {
        prj.load_work_items()
    } else {
        (Vec::new(), Vec::new())
    };
    (feats, wis)
}

fn title_of(feats: &[Feature], wis: &[WorkItem], id: &str) -> Option<String> {
    feats
        .iter()
        .find(|f| f.id() == Some(id))
        .map(|f| f.title.clone())
        .or_else(|| {
            wis.iter()
                .find(|w| w.id() == Some(id))
                .map(|w| w.title.clone())
        })
}

fn has_entry(feats: &[Feature], wis: &[WorkItem], id: &str, field: &str, target: &str) -> bool {
    let fm = feats
        .iter()
        .find(|f| f.id() == Some(id))
        .map(|f| &f.frontmatter)
        .or_else(|| {
            wis.iter()
                .find(|w| w.id() == Some(id))
                .map(|w| &w.frontmatter)
        });
    fm.map(|fm| refs::parse_in(fm, field).iter().any(|(i, _)| i == target))
        .unwrap_or(false)
}

/// Apply `f` to the frontmatter of the doc with `id` (feature or work item),
/// writing it back only if `f` reports a change. Errors if `id` is not found.
fn edit_doc<F>(feats: &mut [Feature], wis: &mut [WorkItem], id: &str, f: F) -> Result<()>
where
    F: FnOnce(&mut Frontmatter) -> bool,
{
    if let Some(feat) = feats.iter_mut().find(|x| x.id() == Some(id)) {
        if f(&mut feat.frontmatter) {
            project::write_feature(feat)?;
        }
        return Ok(());
    }
    if let Some(w) = wis.iter_mut().find(|x| x.id() == Some(id)) {
        if f(&mut w.frontmatter) {
            project::write_work_item(w)?;
        }
        return Ok(());
    }
    Err(OpysError::NotFound { id: id.to_string() })
}
