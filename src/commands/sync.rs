//! The full auto-sync pass: reconcile cross-references, linkify body prose,
//! and regenerate INDEX.md. Invoked by `maybe_sync` after every mutating
//! command and by the `sync` command.

use crate::commands::sync_views;
use crate::error::{usage, Result};
use crate::links;
use crate::project::{self, Project};

pub fn run(prj: &Project) -> Result<()> {
    let (mut docs, errs) = prj.load_docs();
    if !errs.is_empty() {
        return Err(usage("fix parse errors first (run verify)"));
    }

    let orig: Vec<String> = docs.iter().map(|d| d.to_text()).collect();

    links::reconcile(&mut docs);
    links::reconcile_blockers(&mut docs);
    let index = links::build_index(&docs);
    let prefixes: Vec<String> = prj.pcfg.types.values().map(|t| t.prefix.clone()).collect();
    let re = links::ref_re(&prefixes);
    for d in docs.iter_mut() {
        let dir = d.path.parent().unwrap_or(&prj.base).to_path_buf();
        d.body = links::linkify(&d.body, &dir, &index, &re);
    }

    for (d, orig) in docs.iter().zip(&orig) {
        if &d.to_text() != orig {
            project::write_doc(d)?;
        }
    }

    sync_views::regenerate(prj)?;
    Ok(())
}
