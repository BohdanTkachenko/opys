use crate::commands::maybe_sync;
use crate::error::Result;
use crate::frontmatter::Frontmatter;
use crate::project;
use crate::refs;
use crate::Ctx;

/// Strip struck-through (completed) work-item references from every doc. After
/// this the closed work items have no remaining record except git history, and
/// their IDs may be reused.
pub fn run(ctx: &Ctx) -> Result<()> {
    let prj = ctx.open()?;
    prj.require_wi_cfg()?;

    let mut changed = 0usize;
    let (mut feats, _) = prj.load();
    for f in feats.iter_mut() {
        if strip_struck(&mut f.frontmatter) {
            project::write_feature(f)?;
            changed += 1;
        }
    }
    let (mut items, _) = prj.load_work_items();
    for w in items.iter_mut() {
        if strip_struck(&mut w.frontmatter) {
            project::write_work_item(w)?;
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
