//! `opys block` / `opys unblock` — record a directional blocker relationship
//! between two documents of any type.
//!
//! Marking `<id>` blocked by `<by>` writes `<by>` into `<id>`'s `blocked_by` map
//! and `<id>` into `<by>`'s `blocks` map (the bidirectional link). When the
//! blocked document's type has a `blocked` status, it is auto-set — the blocker
//! link itself serves as the `blocked_reason`. A single top-level command pair
//! accepts any id; the prefix resolves the type and file.

use crate::commands::{expand_ids, for_each_id, maybe_sync, touch};
use crate::doc::Doc;
use crate::error::{usage, OpysError, Result};
use crate::frontmatter::Frontmatter;
use crate::project::{self, Project};
use crate::refs;
use crate::Ctx;

/// Mark `id` as blocked by `by`, linking both directions. Does not print or
/// sync — the shared core for the CLI wrapper and the TUI.
pub fn block_core(prj: &Project, id: &str, by: &str) -> Result<()> {
    if id == by {
        return Err(usage("an item cannot block itself"));
    }
    let (mut docs, _) = prj.load_docs();

    let id_title = title_of(&docs, id).ok_or_else(|| OpysError::NotFound { id: id.to_string() })?;
    let by_title = title_of(&docs, by).ok_or_else(|| OpysError::NotFound { id: by.to_string() })?;

    let id_blockable = has_status(prj, id, "blocked");
    edit_doc(prj, &mut docs, id, |fm| {
        let mut changed = refs::add_to_map(fm, refs::BLOCKED_BY, by, &by_title);
        if id_blockable && fm.status() != Some("blocked") {
            fm.set_str("status", "blocked");
            changed = true;
        }
        if changed {
            touch(fm);
        }
        changed
    })?;
    // The reverse `blocks` link on the blocker is derived bookkeeping (the same
    // link reconcile maintains), so it does not bump the blocker's `updated`.
    edit_doc(prj, &mut docs, by, |fm| {
        refs::add_to_map(fm, refs::BLOCKS, id, &id_title)
    })?;
    Ok(())
}

/// Mark one or more documents as blocked by `by`, linking both directions.
pub fn block(ctx: &Ctx, ids: &str, by: &str) -> Result<()> {
    let prj = ctx.open()?;
    let ids = expand_ids(ids)?;
    let res = for_each_id(&ids, |id| {
        block_core(&prj, id, by)?;
        println!("{id} blocked by {by}");
        Ok(())
    });
    maybe_sync(ctx, &prj);
    res
}

/// Remove the blocker link from both sides. When the blocked document's type has
/// a `blocked` status, is left at `blocked` with no remaining blockers and no
/// free-text `blocked_reason`, its status reverts to `in-progress`. Does not
/// print or sync — the shared core for the CLI wrapper and the TUI.
pub fn unblock_core(prj: &Project, id: &str, by: &str) -> Result<()> {
    let (mut docs, _) = prj.load_docs();

    if title_of(&docs, id).is_none() {
        return Err(OpysError::NotFound { id: id.to_string() });
    }
    let linked =
        has_entry(&docs, id, refs::BLOCKED_BY, by) || has_entry(&docs, by, refs::BLOCKS, id);
    if !linked {
        return Err(usage(format!("no blocker '{by}' recorded on '{id}'")));
    }

    let id_blockable = has_status(prj, id, "blocked") && has_status(prj, id, "in-progress");
    edit_doc(prj, &mut docs, id, |fm| {
        let mut changed = refs::remove_from_map(fm, refs::BLOCKED_BY, by);
        if id_blockable
            && fm.status() == Some("blocked")
            && refs::parse_in(fm, refs::BLOCKED_BY).is_empty()
            && fm.get_str("blocked_reason").is_none()
        {
            fm.set_str("status", "in-progress");
            changed = true;
        }
        if changed {
            touch(fm);
        }
        changed
    })?;
    // The blocker may already be gone (a closed doc's struck tombstone); cleaning
    // the target side is enough, so a missing blocker is not an error.
    match edit_doc(prj, &mut docs, by, |fm| {
        refs::remove_from_map(fm, refs::BLOCKS, id)
    }) {
        Ok(()) | Err(OpysError::NotFound { .. }) => {}
        Err(e) => return Err(e),
    }
    Ok(())
}

/// Remove the blocker link from both sides for one or more documents.
pub fn unblock(ctx: &Ctx, ids: &str, by: &str) -> Result<()> {
    let prj = ctx.open()?;
    let ids = expand_ids(ids)?;
    let res = for_each_id(&ids, |id| {
        unblock_core(&prj, id, by)?;
        println!("{id} no longer blocked by {by}");
        Ok(())
    });
    maybe_sync(ctx, &prj);
    res
}

/// Whether the type of `id` declares `status`.
fn has_status(prj: &Project, id: &str, status: &str) -> bool {
    prj.pcfg
        .type_name_for_id(id)
        .map(|n| prj.pcfg.types[n].statuses.iter().any(|s| s == status))
        .unwrap_or(false)
}

fn title_of(docs: &[Doc], id: &str) -> Option<String> {
    docs.iter()
        .find(|d| d.id() == Some(id))
        .map(|d| d.title.clone())
}

fn has_entry(docs: &[Doc], id: &str, field: &str, target: &str) -> bool {
    docs.iter()
        .find(|d| d.id() == Some(id))
        .map(|d| {
            refs::parse_in(&d.frontmatter, field)
                .iter()
                .any(|(i, _)| i == target)
        })
        .unwrap_or(false)
}

/// Apply `f` to the frontmatter of the doc with `id`, writing it back only if
/// `f` reports a change. Errors if `id` is not found.
fn edit_doc<F>(prj: &Project, docs: &mut [Doc], id: &str, f: F) -> Result<()>
where
    F: FnOnce(&mut Frontmatter) -> bool,
{
    if let Some(d) = docs.iter_mut().find(|x| x.id() == Some(id)) {
        if f(&mut d.frontmatter) {
            project::save_doc(prj, d)?;
        }
        return Ok(());
    }
    Err(OpysError::NotFound { id: id.to_string() })
}
