//! `opys cleanup` — strip struck-through (closed) references from every
//! document. After this the closed documents have no remaining record except
//! git history.

use crate::commands::maybe_sync;
use crate::error::Result;
use crate::frontmatter::Frontmatter;
use crate::{refs, Ctx};

pub fn run(ctx: &Ctx) -> Result<()> {
    let prj = ctx.open()?;
    let (mut store, _) = ctx.load(&prj)?;
    let mut changed = 0usize;
    for (dkey, mut doc) in store.all_docs()? {
        if strip_struck(&mut doc.frontmatter) {
            store.put_doc(&prj.pcfg, Some(dkey), &doc)?;
            changed += 1;
        }
    }
    ctx.flush(&prj, store)?;
    println!("cleanup: removed struck references from {changed} doc(s)");
    maybe_sync(ctx, &prj);
    Ok(())
}

fn strip_struck(fm: &mut Frontmatter) -> bool {
    let mut changed = false;
    for field in refs::RELATION_FIELDS {
        let entries = refs::parse_in(fm, field);
        let kept: Vec<_> = entries
            .iter()
            .filter(|(_, v)| !refs::is_struck(v))
            .cloned()
            .collect();
        if kept.len() != entries.len() {
            refs::set_in(fm, field, &kept);
            changed = true;
        }
    }
    changed
}
