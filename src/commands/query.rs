//! `opys query` — run SQL over the live corpus store.
//!
//! Read-only by default: a plan-guarded SELECT (nothing else can execute), and
//! the command never flushes, so the files are unreachable. With `--write`,
//! INSERT/UPDATE/DELETE run against the store, the normal sync pass reconciles
//! and relocates, and the result is validated with `verify` — the files are
//! written **only if verify passes**. So a raw SQL edit gets full power but can
//! never leave the inventory in a state the CLI would reject; a corrupting edit
//! changes nothing on disk (the store mutation is in-memory and simply not
//! flushed).

use std::io::Read as _;

use crate::commands::verify;
use crate::doc::Doc;
use crate::error::{usage, Result};
use crate::store::Store;
use crate::Ctx;

use super::stats;

pub fn run(ctx: &Ctx, sql: &str, plain: bool, write: bool) -> Result<()> {
    let prj = ctx.open()?;
    let sql = if sql == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        sql.to_string()
    };
    if sql.trim().is_empty() {
        return Err(usage("query: empty SQL"));
    }

    // Unparsable docs are warnings — query the parsable subset (list parity).
    let (mut store, errors) = Store::open(&prj)?;
    for e in &errors {
        eprintln!("warning: {e}");
    }
    store.refresh_projections(&prj.pcfg)?;

    if write {
        // The gate is "introduce no NEW verify problem", not "pass verify" — a
        // corpus routinely carries transient issues (e.g. a test ref not yet
        // written), and a bulk edit shouldn't be blocked by pre-existing ones.
        // So snapshot the problems before the write and compare.
        let before: std::collections::HashSet<String> = {
            let docs = docs_of(&mut store)?;
            verify::collect_problems(&prj, &docs, errors.clone())
                .into_iter()
                .collect()
        };

        let summary = store.run_user_write(&sql).map_err(usage)?;
        if !ctx.no_sync {
            crate::commands::sync::pass(&prj, &mut store)?;
        }

        let after = verify::collect_problems(&prj, &docs_of(&mut store)?, errors);
        let new: Vec<String> = after.into_iter().filter(|p| !before.contains(p)).collect();
        if !new.is_empty() {
            eprintln!("refusing to write — the edit would introduce verify problems:");
            for p in &new {
                eprintln!("  {p}");
            }
            return Err(usage(format!(
                "{} new problem(s); no files changed",
                new.len()
            )));
        }
        store.flush(&prj)?;
        println!("query: {summary} (verified, written)");
        return Ok(());
    }

    let (labels, rows) = store.run_user_query(&sql).map_err(usage)?;
    stats::print_markdown(&stats::table_body(&labels, &rows), plain);
    Ok(())
}

/// All reconstructed docs (the shape verify's checks consume).
fn docs_of(store: &mut Store) -> Result<Vec<Doc>> {
    Ok(store.all_docs()?.into_iter().map(|(_, d)| d).collect())
}
