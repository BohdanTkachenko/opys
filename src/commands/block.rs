//! `opys block` / `opys unblock` — record a directional blocker relationship
//! between two documents of any type, through the corpus store.
//!
//! Marking `<id>` blocked by `<by>` writes `<by>` into `<id>`'s `blocked_by` map
//! and `<id>` into `<by>`'s `blocks` map (the bidirectional link). That is the
//! whole write: blocked is *derived* from the map, never stored (ADR-0050
//! decision 3), so neither command touches `status` and neither appends to
//! `history`.

use crate::commands::{apply_batch, maybe_sync, print_lines, resolve_ids, touch};
use crate::error::{usage, OpysError, Result};
use crate::frontmatter::Frontmatter;
use crate::project::Project;
use crate::refs;
use crate::store::Store;
use crate::Ctx;

/// Mark `id` as blocked by `by`, linking both directions. Does not flush/print/
/// sync — the shared core for the CLI wrapper and the node's typed action.
pub fn block_core(prj: &Project, store: &mut Store, id: &str, by: &str) -> Result<()> {
    if id == by {
        return Err(usage("an item cannot block itself"));
    }
    let id_title = store
        .title_of(id)?
        .ok_or_else(|| OpysError::NotFound { id: id.to_string() })?;
    let by_title = store
        .title_of(by)?
        .ok_or_else(|| OpysError::NotFound { id: by.to_string() })?;

    edit_doc(prj, store, id, |fm| {
        let changed = refs::add_to_map(fm, refs::BLOCKED_BY, by, &by_title);
        if changed {
            touch(fm);
        }
        changed
    })?;
    // The reverse `blocks` link on the blocker is derived bookkeeping (the same
    // link reconcile maintains), so it does not bump the blocker's `updated`.
    edit_doc(prj, store, by, |fm| {
        refs::add_to_map(fm, refs::BLOCKS, id, &id_title)
    })?;
    Ok(())
}

/// Mark one or more documents as blocked by `by`, linking both directions.
pub fn block(ctx: &Ctx, ids: &[String], by: &str) -> Result<()> {
    let prj = ctx.open()?;
    let ids = resolve_ids(ids)?;
    let (mut store, _) = ctx.load(&prj)?;
    let lines = apply_batch(&ids, |id| {
        block_core(&prj, &mut store, id, by)?;
        Ok(format!("{id} blocked by {by}"))
    })?;
    ctx.flush(&prj, store)?;
    print_lines(lines)?;
    maybe_sync(ctx, &prj);
    Ok(())
}

/// Remove the blocker link from both sides. Like `block`, it writes the two maps
/// and nothing else: the document stops being blocked because the entry is gone,
/// not because anything set a status. Does not flush/print/sync — the shared
/// core for the CLI and the node's typed action.
pub fn unblock_core(prj: &Project, store: &mut Store, id: &str, by: &str) -> Result<()> {
    if store.dkey_opt(id)?.is_none() {
        return Err(OpysError::NotFound { id: id.to_string() });
    }
    let linked = store.has_relation(id, refs::BLOCKED_BY, by)?
        || store.has_relation(by, refs::BLOCKS, id)?;
    if !linked {
        return Err(usage(format!("no blocker '{by}' recorded on '{id}'")));
    }

    edit_doc(prj, store, id, |fm| {
        let changed = refs::remove_from_map(fm, refs::BLOCKED_BY, by);
        if changed {
            touch(fm);
        }
        changed
    })?;
    // The blocker may already be gone (a closed doc's struck tombstone); cleaning
    // the target side is enough, so a missing blocker is not an error.
    match edit_doc(prj, store, by, |fm| {
        refs::remove_from_map(fm, refs::BLOCKS, id)
    }) {
        Ok(()) | Err(OpysError::NotFound { .. }) => {}
        Err(e) => return Err(e),
    }
    Ok(())
}

/// Remove the blocker link from both sides for one or more documents.
pub fn unblock(ctx: &Ctx, ids: &[String], by: &str) -> Result<()> {
    let prj = ctx.open()?;
    let ids = resolve_ids(ids)?;
    let (mut store, _) = ctx.load(&prj)?;
    let lines = apply_batch(&ids, |id| {
        unblock_core(&prj, &mut store, id, by)?;
        Ok(format!("{id} no longer blocked by {by}"))
    })?;
    ctx.flush(&prj, store)?;
    print_lines(lines)?;
    maybe_sync(ctx, &prj);
    Ok(())
}

/// Apply `f` to the frontmatter of the doc with `id`, writing it back only if
/// `f` reports a change. Errors if `id` is not found.
///
/// Neither a relocation flag nor a fallible closure any more: a document's
/// canonical path is a function of its status and these two commands no longer
/// change one, and the only guard that had to run against loaded frontmatter was
/// the lifecycle-map check that protected the auto-set.
fn edit_doc<F>(prj: &Project, store: &mut Store, id: &str, f: F) -> Result<()>
where
    F: FnOnce(&mut Frontmatter) -> bool,
{
    let dkey = store.dkey_of(id)?;
    let mut doc = store.doc(dkey)?;
    if f(&mut doc.frontmatter) {
        store.put_doc(&prj.pcfg, Some(dkey), &doc)?;
    }
    Ok(())
}
