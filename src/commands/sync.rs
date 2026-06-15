//! The full auto-sync pass: reconcile cross-references, linkify body prose,
//! and regenerate INDEX.md/views. Invoked by `maybe_sync` after every mutating
//! command and by the `sync-views` command.

use crate::commands::sync_views;
use crate::error::{usage, Result};
use crate::links;
use crate::project::{self, Project};

pub fn run(prj: &Project) -> Result<()> {
    let (mut feats, ferr) = prj.load();
    if !ferr.is_empty() {
        return Err(usage("fix parse errors first (run verify)"));
    }
    let (mut live, werr) = if prj.wi_cfg.is_some() {
        prj.load_work_items()
    } else {
        (Vec::new(), Vec::new())
    };
    if !werr.is_empty() {
        return Err(usage("fix work-item parse errors first (run verify)"));
    }

    let orig_f: Vec<String> = feats.iter().map(|f| f.to_text()).collect();
    let orig_w: Vec<String> = live.iter().map(|w| w.to_text()).collect();

    links::reconcile(&mut feats, &mut live);
    links::reconcile_blockers(&mut feats, &mut live);
    let index = links::build_index(&feats, &live);
    for f in feats.iter_mut() {
        let dir = f.path.parent().unwrap_or(&prj.fdir).to_path_buf();
        f.body = links::linkify(&f.body, &dir, &index);
    }
    for w in live.iter_mut() {
        let dir = w.path.parent().unwrap_or(&prj.wdir).to_path_buf();
        w.body = links::linkify(&w.body, &dir, &index);
    }

    for (f, orig) in feats.iter().zip(&orig_f) {
        if &f.to_text() != orig {
            project::write_feature(f)?;
        }
    }
    for (w, orig) in live.iter().zip(&orig_w) {
        if &w.to_text() != orig {
            project::write_work_item(w)?;
        }
    }

    sync_views::regenerate(prj)?;
    Ok(())
}
