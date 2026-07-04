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
        let baseline = store.baseline()?;
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
        store.cascade_removals(&prj.pcfg, &baseline, &super::today())?;
        materialize_inserts(&prj, &mut store)?;
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

/// Materialize documents created by a raw `INSERT INTO docs`. The store assigns
/// a `dkey` to every row it inserts, so a NULL-`dkey` row is a user INSERT: give
/// it the next id for its type (unless it supplied one), default its status,
/// stamp timestamps, scaffold the body for its type, and re-insert it
/// canonically. Tags/relations aren't set here — add them via the CLI or
/// follow-up SQL. Runs before the verify gate, so a malformed insert is refused
/// with no files changed.
fn materialize_inserts(prj: &crate::project::Project, store: &mut Store) -> Result<()> {
    use crate::store::g_str;
    let (_, rows) = store.select(
        "SELECT id, type, status, title, body FROM docs WHERE dkey IS NULL",
        vec![],
    )?;
    if rows.is_empty() {
        return Ok(());
    }
    let pcfg = &prj.pcfg;

    // Extract + validate every inserted row before mutating the store, so an
    // invalid INSERT changes nothing on disk.
    struct Raw {
        id: Option<String>,
        type_name: String,
        status: String,
        title: String,
        body: String,
    }
    let mut raws = Vec::with_capacity(rows.len());
    for r in &rows {
        let type_name = g_str(&r[1]).unwrap_or_default();
        let t = pcfg.types.get(&type_name).ok_or_else(|| {
            usage(format!(
                "query: INSERT into docs needs a known type (got {type_name:?})"
            ))
        })?;
        let status = {
            let s = g_str(&r[2]).unwrap_or_default();
            if s.is_empty() {
                t.default_status.clone()
            } else {
                s
            }
        };
        if !t.statuses.contains(&status) {
            return Err(usage(format!(
                "query: unknown status {status:?} for type {type_name:?}"
            )));
        }
        let title = g_str(&r[3]).unwrap_or_default();
        if title.trim().is_empty() {
            return Err(usage(
                "query: INSERT into docs needs a non-empty title".to_string(),
            ));
        }
        raws.push(Raw {
            id: g_str(&r[0]).filter(|s| !s.is_empty()),
            type_name,
            status,
            title,
            body: g_str(&r[4]).unwrap_or_default(),
        });
    }

    // Swap the raw partial rows for canonical documents. Allocate ids one at a
    // time so multiple inserts in one statement get distinct sequential ids.
    store.exec("DELETE FROM docs WHERE dkey IS NULL", vec![])?;
    for raw in raws {
        let t = &pcfg.types[&raw.type_name];
        let id = match raw.id {
            Some(id) => id,
            None => store.next_id_for(&t.prefix, pcfg.pad)?,
        };
        let mut fm = crate::frontmatter::Frontmatter::new();
        fm.set_str("id", &id);
        fm.set_str("status", &raw.status);
        crate::commands::touch(&mut fm);
        let body = if raw.body.trim().is_empty() {
            crate::commands::new::scaffold_body(&raw.title, t)
        } else {
            raw.body
        };
        let doc = Doc {
            path: prj.doc_path(&id, &raw.status),
            frontmatter: fm,
            body,
            title: raw.title,
        };
        store.put_doc(pcfg, None, &doc)?;
    }
    Ok(())
}
