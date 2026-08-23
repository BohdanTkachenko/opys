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
use crate::ids::IdSource;
use crate::store::Store;
use crate::Ctx;

use super::stats;

pub fn run(ctx: &Ctx, sql: &str, plain: bool, write: bool, stdin_value: bool) -> Result<()> {
    let prj = ctx.open()?;
    let sql_from_stdin = sql == "-";
    let sql = if sql_from_stdin {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        sql.to_string()
    };
    if sql.trim().is_empty() {
        return Err(usage("query: empty SQL"));
    }
    // `--stdin` binds stdin to `$1` in the SQL (large/multi-line values without
    // SQL-escaping); it can't also read the SQL itself from stdin.
    let params: Vec<String> = if stdin_value {
        if sql_from_stdin {
            return Err(usage(
                "query: --stdin cannot combine with reading the SQL from stdin (-)",
            ));
        }
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        vec![buf.trim_end().to_string()]
    } else {
        Vec::new()
    };

    // Unparsable docs are warnings — query the parsable subset (list parity).
    let (mut store, errors) = ctx.load(&prj)?;
    for e in &errors {
        eprintln!("warning: {e}");
    }
    store.refresh_projections(&prj.pcfg)?;

    if write {
        let baseline = store.baseline()?;
        let rel_before = store.relation_entries()?;
        let before_blocks = block_texts(&mut store)?;
        // Three seams: (1) apply the user's intent as raw DML, (2) the model
        // pass reconciles the raw tables into a consistent model (cascades +
        // sync), (3) the validator gates the result. Only edits that make
        // verify *worse* are refused, so pre-existing issues never block a
        // write. A future typed data model would replace seam 3 alone.
        let before = verify_problems(&prj, &mut store, &errors)?;

        let summary = store.run_user_write(&sql, &params).map_err(usage)?;
        apply_block_edits(&prj, &mut store, &before_blocks)?;
        reconcile_model(&prj, &mut store, &baseline, &rel_before, !ctx.no_sync)?;

        let after = verify_problems(&prj, &mut store, &errors)?;
        let new: Vec<String> = after.difference(&before).cloned().collect();
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
        ctx.flush(&prj, store)?;
        println!("query: {summary} (verified, written)");
        return Ok(());
    }

    let (labels, rows) = store.run_user_query(&sql, &params).map_err(usage)?;
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
            None => {
                let n = crate::ids::SequenceMax.reserve(store, 1)?;
                crate::ids::format_id(&t.prefix, n, pcfg.pad)
            }
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

/// The reactive model pass: bring the raw tables — freshly mutated by the user's
/// DML — to a consistent model by materializing the lifecycle cascades a raw
/// column edit can't express, then running the normal sync reconcile/linkify/
/// relocate. It reacts to STATE deltas, never to the SQL text (shape-parsing is
/// unreliable). Deliberately separate from validation (see [`verify_problems`])
/// so a future typed data model can replace the validator without touching
/// these cascades.
fn reconcile_model(
    prj: &crate::project::Project,
    store: &mut Store,
    baseline: &crate::store::Baseline,
    rel_before: &[(String, String)],
    run_sync: bool,
) -> Result<()> {
    // Removals: strike inbound references + reserve ids for docs the edit dropped.
    store.cascade_removals(&prj.pcfg, baseline)?;
    // A relation row the edit deleted may have been an id's only anchor (e.g.
    // the tombstone of a closed doc) — re-reserve any pre-edit entry that is
    // now anchored nowhere.
    store.reserve_unanchored(rel_before)?;
    // Inserts: turn raw `INSERT INTO docs` rows into allocated, scaffolded docs.
    materialize_inserts(prj, store)?;
    if run_sync {
        crate::commands::sync::pass(prj, store)?;
    }
    Ok(())
}

/// The validation seam: the current corpus's verify problems as a set. The
/// write gate compares this before vs. after the model pass and refuses any
/// edit that introduces a NEW problem — pre-existing issues never block an edit.
/// Problems verify prefixes with a document *path* are re-keyed to the doc's id
/// (paths are mutable — an edit that relocates a file must not make its
/// pre-existing problems look new). A future typed data model would replace
/// THIS function, leaving the model pass (see [`reconcile_model`]) untouched.
fn verify_problems(
    prj: &crate::project::Project,
    store: &mut Store,
    errors: &[String],
) -> Result<std::collections::HashSet<String>> {
    let docs = docs_of(store)?;
    let id_by_path: Vec<(String, String)> = docs
        .iter()
        .filter_map(|d| Some((d.path.display().to_string(), d.id()?.to_string())))
        .collect();
    Ok(verify::collect_problems(prj, &docs, errors.to_vec())
        .into_iter()
        .map(|p| rekey_by_id(p, &id_by_path))
        .collect())
}

/// Replace every known doc path in a problem message with the doc's id — the
/// `"<path>: …"` head and any path embedded in the tail (e.g. the duplicate-id
/// message's `(also in <path>)`).
fn rekey_by_id(mut problem: String, id_by_path: &[(String, String)]) -> String {
    for (path, id) in id_by_path {
        if problem.contains(path.as_str()) {
            problem = problem.replace(path.as_str(), id);
        }
    }
    problem
}

/// Snapshot the `blocks` projection's section texts (`(doc_id, seq) -> text`),
/// taken before user DML so [`apply_block_edits`] can find which sections were
/// edited.
fn block_texts(store: &mut Store) -> Result<std::collections::HashMap<(String, i64), String>> {
    let (_, rows) = store.select("SELECT doc_id, seq, text FROM blocks", vec![])?;
    let mut map = std::collections::HashMap::new();
    for r in &rows {
        if let (Some(id), Some(seq)) = (crate::store::g_str(&r[0]), crate::store::g_i64(&r[1])) {
            map.insert((id, seq), crate::store::g_str(&r[2]).unwrap_or_default());
        }
    }
    Ok(map)
}

/// Apply `UPDATE blocks SET text = …` edits back to the authoritative body: for
/// every section whose text changed vs. `before`, splice the new content into
/// the doc's body (byte-accurately, via `body::section_spans`) and bump its
/// `updated`. Other `blocks` columns (heading/seq) and INSERT/DELETE on `blocks`
/// are no-ops — the body is edited through `text` only.
fn apply_block_edits(
    prj: &crate::project::Project,
    store: &mut Store,
    before: &std::collections::HashMap<(String, i64), String>,
) -> Result<()> {
    let (_, rows) = store.select("SELECT doc_id, seq, text FROM blocks", vec![])?;
    let mut by_doc: std::collections::BTreeMap<String, Vec<(usize, String)>> =
        std::collections::BTreeMap::new();
    for r in &rows {
        let (Some(id), Some(seq)) = (crate::store::g_str(&r[0]), crate::store::g_i64(&r[1])) else {
            continue;
        };
        let text = crate::store::g_str(&r[2]).unwrap_or_default();
        if before.get(&(id.clone(), seq)).is_some_and(|t| *t != text) {
            by_doc.entry(id).or_default().push((seq as usize, text));
        }
    }
    if by_doc.is_empty() {
        return Ok(());
    }
    for (doc_id, edits) in by_doc {
        let Some(dkey) = store.dkey_opt(&doc_id)? else {
            continue;
        };
        let mut doc = store.doc(dkey)?;
        let spans = crate::body::section_spans(&doc.body);
        doc.body = crate::body::apply_section_edits(&doc.body, &spans, &edits);
        doc.title = crate::body::title(&doc.body);
        super::touch(&mut doc.frontmatter);
        store.put_doc(&prj.pcfg, Some(dkey), &doc)?;
    }
    Ok(())
}
