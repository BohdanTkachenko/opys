//! The full auto-sync pass: reconcile cross-references, linkify body prose, and
//! relocate documents to their canonical layout path. Invoked by `maybe_sync`
//! after every mutating command and by the `opys sync` command.

use crate::error::{usage, Result};
use crate::links;
use crate::project::{self, Project};
use crate::Ctx;

/// The `opys sync` command entry: open the project and run the full pass.
pub fn run_command(ctx: &Ctx) -> Result<()> {
    let prj = ctx.open()?;
    let n = run(&prj)?;
    println!("synced {n} document(s)");
    Ok(())
}

/// Reconcile relations, linkify prose, and relocate docs to their canonical
/// layout paths; write back every document that changed or is mislocated.
/// Returns the number of documents. Errors (without writing) if any document
/// fails to parse.
pub fn run(prj: &Project) -> Result<usize> {
    let (mut docs, errs) = prj.load_docs();
    if !errs.is_empty() {
        return Err(usage("fix parse errors first (run verify)"));
    }
    // Clean up the index older versions generated at the base; opys no longer
    // writes one (slice the inventory live with `opys list` instead).
    let _ = std::fs::remove_file(prj.base.join("INDEX.md"));

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

    let count = docs.len();
    for (d, orig) in docs.iter_mut().zip(&orig) {
        // Write when content changed, or when the file is mislocated relative to
        // the configured layout (e.g. after a status change or a layout edit) —
        // `save_doc` relocates it to its canonical path.
        let canonical = match (d.id(), d.status()) {
            (Some(id), Some(status)) => Some(prj.doc_path(id, status)),
            _ => None,
        };
        let mislocated = canonical.as_ref().is_some_and(|c| c != &d.path);
        if &d.to_text() != orig || mislocated {
            project::save_doc(prj, d)?;
        }
    }
    Ok(count)
}
