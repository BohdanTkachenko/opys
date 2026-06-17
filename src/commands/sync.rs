//! The full auto-sync pass: reconcile cross-references, linkify body prose, and
//! relocate documents to their canonical layout path. Invoked by `maybe_sync`
//! after every mutating command and by the `opys sync` command.

use std::path::Path;

use time::{format_description::well_known::Rfc3339, OffsetDateTime};

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

    // Backfill the auto-maintained timestamps on docs predating the fields, from
    // the file's mtime. This is housekeeping, not a user edit, so it must not
    // bump `updated` on docs that already have it — only fill genuine gaps.
    for d in docs.iter_mut() {
        let need_created = !d.frontmatter.contains_key("created");
        let need_updated = !d.frontmatter.contains_key("updated");
        if need_created || need_updated {
            if let Some(ts) = mtime_rfc3339(&d.path) {
                if need_created {
                    d.frontmatter.set_str("created", ts.clone());
                }
                if need_updated {
                    d.frontmatter.set_str("updated", ts);
                }
            }
        }
    }

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

/// A file's modification time as an RFC3339 datetime (second precision), or
/// `None` if it cannot be read. Used to backfill `created`/`updated` on docs
/// that predate those fields.
fn mtime_rfc3339(path: &Path) -> Option<String> {
    let mt = std::fs::metadata(path).ok()?.modified().ok()?;
    let dt = OffsetDateTime::from(mt);
    let dt = dt.replace_nanosecond(0).unwrap_or(dt);
    dt.format(&Rfc3339).ok()
}
