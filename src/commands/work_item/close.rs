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

    // Features the work item references should carry a struck `references`
    // tombstone even if their reverse link is not yet present.
    let ref_targets: HashSet<String> = refs::parse(&wi.frontmatter)
        .into_iter()
        .map(|(tid, _)| tid)
        .collect();

    let (mut feats, _) = prj.load();
    for f in feats.iter_mut() {
        let add_ref = ref_targets.contains(f.id().unwrap_or(""));
        if strike_everywhere(&mut f.frontmatter, id, &struck, add_ref) {
            project::write_feature(f)?;
        }
    }

    let (mut others, _) = prj.load_work_items();
    for w in others.iter_mut() {
        if w.id() == Some(id) {
            continue;
        }
        let add_ref = ref_targets.contains(w.id().unwrap_or(""));
        if strike_everywhere(&mut w.frontmatter, id, &struck, add_ref) {
            project::write_work_item(w)?;
        }
    }

    std::fs::remove_file(&wi_path)?;
    println!("closed {id} (deleted; references struck through)");
    maybe_sync(ctx, &prj);
    Ok(())
}

/// Strike `id` to a tombstone in every relation map (`references`,
/// `blocked_by`, `blocks`) that lists it; for `references`, optionally add a
/// struck entry when `add_ref` and none is present. Returns whether anything
/// changed.
fn strike_everywhere(fm: &mut Frontmatter, id: &str, struck: &str, add_ref: bool) -> bool {
    let mut changed = false;
    for field in refs::RELATION_FIELDS {
        let add = add_ref && field == refs::FIELD;
        changed |= strike_in_field(fm, field, id, struck, add);
    }
    changed
}

/// Strike `id`'s value in one relation field. Adds the entry (struck) when
/// `add_if_missing` and it is absent. Returns whether the field changed.
fn strike_in_field(
    fm: &mut Frontmatter,
    field: &str,
    id: &str,
    struck: &str,
    add_if_missing: bool,
) -> bool {
    let mut entries = refs::parse_in(fm, field);
    if let Some(e) = entries.iter_mut().find(|(i, _)| i == id) {
        if e.1 == struck {
            return false;
        }
        e.1 = struck.to_string();
    } else if add_if_missing {
        entries.push((id.to_string(), struck.to_string()));
    } else {
        return false;
    }
    refs::set_in(fm, field, &entries);
    true
}
