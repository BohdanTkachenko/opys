//! The full auto-sync pass: reconcile cross-references, linkify body prose, and
//! relocate documents to their canonical layout path. Invoked by `maybe_sync`
//! after every mutating command and by the `opys sync` command.
//!
//! The pass runs over the in-memory store: reconstruct every doc, backfill
//! timestamps, run the (unchanged) `links` reconcile/linkify engine on the
//! reconstructed `Doc`s, write the changed ones back via `put_doc`, point every
//! doc at its canonical path, and flush. Relation-map / linkify logic stays in
//! Rust — the store is the data layer feeding it.

use crate::doc::Doc;
use crate::error::{usage, Result};
use crate::links;
use crate::project::Project;
use crate::store::Store;
use crate::Ctx;

/// The `opys sync` command entry: open the project and run the full pass.
pub fn run_command(ctx: &Ctx) -> Result<()> {
    let prj = ctx.open()?;
    let n = run(&prj)?;
    println!("synced {n} document(s)");
    Ok(())
}

/// Load the corpus, run the sync pass, and flush. Returns the document count.
/// Errors (without writing) if any document fails to parse.
pub fn run(prj: &Project) -> Result<usize> {
    let (mut store, errs) = Store::open(prj)?;
    if !errs.is_empty() {
        return Err(usage("fix parse errors first (run verify)"));
    }
    let n = pass(prj, &mut store)?;
    store.flush(prj)?;
    Ok(n)
}

/// Run the reconcile/linkify/backfill/relocate pass against an already-open
/// store (no flush). Returns the document count. Used both by [`run`] and, on
/// the invocation's own store, after a mutating command.
pub fn pass(prj: &Project, store: &mut Store) -> Result<usize> {
    // Clean up the index older versions generated at the base; opys no longer
    // writes one (slice the inventory live with `opys list`/`opys query`).
    let _ = std::fs::remove_file(prj.base.join("INDEX.md"));

    let rows = store.full_rows()?;
    let count = rows.len();

    // Reconstruct the working set, backfilling the auto-maintained timestamps on
    // docs predating the fields from the file's mtime. Housekeeping, so it must
    // not bump `updated` on docs that already have it — only fill genuine gaps.
    let mut docs: Vec<Doc> = Vec::with_capacity(count);
    for r in &rows {
        let mut doc = r.doc.clone();
        let need_created = !doc.frontmatter.contains_key("created");
        let need_updated = !doc.frontmatter.contains_key("updated");
        if (need_created || need_updated) && r.orig_mtime.is_some() {
            let ts = r.orig_mtime.clone().unwrap();
            if need_created {
                doc.frontmatter.set_str("created", ts.clone());
            }
            if need_updated {
                doc.frontmatter.set_str("updated", ts);
            }
        }
        docs.push(doc);
    }

    // The (unchanged) relation/linkify engine operates on the reconstructed set.
    links::reconcile(&mut docs);
    links::reconcile_blockers(&mut docs);
    let index = links::build_index(&docs);
    let prefixes: Vec<String> = prj.pcfg.types.values().map(|t| t.prefix.clone()).collect();
    let re = links::ref_re(&prefixes);
    for d in docs.iter_mut() {
        let dir = d.path.parent().unwrap_or(&prj.base).to_path_buf();
        d.body = links::linkify(&d.body, &dir, &index, &re);
    }

    // Write back what the pass changed, and point every mislocated doc at its
    // canonical path (flush relocates the file even with no content change).
    // The canonical path is computed in Rust and only written when it differs,
    // so the common no-op sync issues almost no SQL.
    for (r, d) in rows.iter().zip(&docs) {
        if d.to_text() != r.doc.to_text() {
            store.put_doc(&prj.pcfg, Some(r.dkey), d)?;
        }
        if let (Some(id), Some(status)) = (d.id(), d.status()) {
            let canonical = prj
                .pcfg
                .doc_relpath(id, status)
                .to_string_lossy()
                .into_owned();
            if canonical != r.relpath {
                store.set_path(r.dkey, &canonical)?;
            }
        }
    }
    Ok(count)
}
