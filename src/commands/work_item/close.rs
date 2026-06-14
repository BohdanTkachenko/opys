use std::collections::HashSet;

use crate::body;
use crate::commands::maybe_sync;
use crate::error::{usage, Result};
use crate::frontmatter::Frontmatter;
use crate::project;
use crate::refs;
use crate::Ctx;

pub fn run(ctx: &Ctx, id: &str, force: bool) -> Result<()> {
    let prj = ctx.open()?;
    prj.require_wi_cfg()?;
    let (items, _) = prj.load_work_items();
    let wi = prj.find_wi(&items, id)?;
    let wi_path = wi.path.clone();
    let struck = refs::strike(&wi.title);

    // Don't close with unfinished work unless forced.
    let tasks = body::checklist_items(&wi.body, "Tasks");
    if !force && tasks.iter().any(|t| !t.checked) {
        return Err(usage(
            "cannot close — unchecked tasks remain (check them, or pass --force)",
        ));
    }

    // Docs that should carry the struck tombstone: those the work item links,
    // plus any doc that already references it.
    let targets: HashSet<String> = refs::parse(&wi.frontmatter)
        .into_iter()
        .map(|(tid, _)| tid)
        .collect();

    let (mut feats, _) = prj.load();
    for f in feats.iter_mut() {
        let fid = f.id().unwrap_or("").to_string();
        if (targets.contains(&fid) || references(&f.frontmatter, id))
            && ensure_struck(&mut f.frontmatter, id, &struck)
        {
            project::write_feature(f)?;
        }
    }

    let (mut others, _) = prj.load_work_items();
    for w in others.iter_mut() {
        if w.id() == Some(id) {
            continue;
        }
        let wid = w.id().unwrap_or("").to_string();
        if (targets.contains(&wid) || references(&w.frontmatter, id))
            && ensure_struck(&mut w.frontmatter, id, &struck)
        {
            project::write_work_item(w)?;
        }
    }

    std::fs::remove_file(&wi_path)?;
    println!("closed {id} (deleted; reference struck through)");
    maybe_sync(ctx, &prj);
    Ok(())
}

fn references(fm: &Frontmatter, id: &str) -> bool {
    refs::parse(fm).iter().any(|(i, _)| i == id)
}

/// Ensure `fm` references `id` with a struck-through value. Returns whether the
/// frontmatter changed.
fn ensure_struck(fm: &mut Frontmatter, id: &str, struck: &str) -> bool {
    let mut entries = refs::parse(fm);
    if let Some(e) = entries.iter_mut().find(|(i, _)| i == id) {
        if e.1 == struck {
            return false;
        }
        e.1 = struck.to_string();
    } else {
        entries.push((id.to_string(), struck.to_string()));
    }
    refs::set(fm, &entries);
    true
}
