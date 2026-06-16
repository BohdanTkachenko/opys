//! `opys cleanup` — strip struck-through (closed) references from every
//! document. After this the closed documents have no remaining record except
//! git history.

use crate::commands::maybe_sync;
use crate::error::Result;
use crate::frontmatter::Frontmatter;
use crate::{project, refs, Ctx};

pub fn run(ctx: &Ctx) -> Result<()> {
    let prj = ctx.open()?;
    let mut changed = 0usize;
    let (mut docs, _) = prj.load_docs();
    for d in docs.iter_mut() {
        if strip_struck(&mut d.frontmatter) {
            project::save_doc(&prj, d)?;
            changed += 1;
        }
    }
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
