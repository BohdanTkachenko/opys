//! `opys cleanup` — strip struck-through (closed) references from every
//! document. After this the closed documents have no remaining record except
//! git history and the retired ledger: any id whose last remaining anchor was a
//! stripped tombstone is reserved in `_retired.md` first, so the global id
//! sequence still never reissues it.

use crate::commands::maybe_sync;
use crate::error::Result;
use crate::frontmatter::Frontmatter;
use crate::{refs, Ctx};

pub fn run(ctx: &Ctx) -> Result<()> {
    let prj = ctx.open()?;
    let (mut store, _) = ctx.load(&prj)?;
    let mut changed = 0usize;
    let mut stripped: Vec<(String, String)> = Vec::new();
    for (dkey, mut doc) in store.all_docs()? {
        let removed = strip_struck(&mut doc.frontmatter);
        if !removed.is_empty() {
            store.put_doc(&prj.pcfg, Some(dkey), &doc)?;
            changed += 1;
            stripped.extend(removed);
        }
    }
    // Docs closed before the ledger-on-close era are reserved only by their
    // tombstones — re-anchor those ids in the ledger before the flush that
    // removes the tombstones.
    store.reserve_unanchored(&stripped)?;
    ctx.flush(&prj, store)?;
    println!("cleanup: removed struck references from {changed} doc(s)");
    maybe_sync(ctx, &prj);
    Ok(())
}

/// Remove every struck entry from the relation maps, returning the stripped
/// `(id, unstruck title)` pairs (empty = nothing changed).
fn strip_struck(fm: &mut Frontmatter) -> Vec<(String, String)> {
    let mut removed = Vec::new();
    for field in refs::RELATION_FIELDS {
        let entries = refs::parse_in(fm, field);
        let (gone, kept): (Vec<_>, Vec<_>) =
            entries.into_iter().partition(|(_, v)| refs::is_struck(v));
        if !gone.is_empty() {
            refs::set_in(fm, field, &kept);
            removed.extend(
                gone.into_iter()
                    .map(|(id, v)| (id, refs::unstrike(&v).to_string())),
            );
        }
    }
    removed
}
