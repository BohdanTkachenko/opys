//! `opys set-status` — a guarded status transition through the corpus store.
//!
//! The mutation pattern shared by every scalar mutator: reconstruct the doc,
//! apply the change with the existing frontmatter/`touch` helpers, validate
//! (rules engine) on that transient clone, and only then write it back via
//! `Store::put_doc` (which re-decomposes into the tables). Guards run before
//! any write, so a failed id leaves the store untouched.

use crate::commands::{apply_batch, maybe_sync, print_lines, resolve_ids, touch};
use crate::error::{usage, Result};
use crate::project::Project;
use crate::rules;
use crate::store::Store;
use crate::Ctx;

/// Apply a status transition to the store. Does not flush/print/sync.
pub fn core(
    prj: &Project,
    store: &mut Store,
    id: &str,
    status: &str,
    reason: Option<&str>,
) -> Result<()> {
    let pcfg = &prj.pcfg;
    let tname = pcfg
        .type_name_for_id(id)
        .ok_or_else(|| usage(format!("unrecognized id prefix in {id}")))?
        .to_string();
    let t = &pcfg.types[&tname];

    if !t.statuses.iter().any(|s| s == status) {
        return Err(usage(format!(
            "unknown status {status:?} for type '{tname}' (allowed: {})",
            t.statuses.join(", ")
        )));
    }
    if t.terminal_statuses.iter().any(|s| s == status) {
        return Err(usage(format!(
            "{status} is terminal — use `opys close {id}` to reach it"
        )));
    }

    let dkey = store.dkey_of(id)?;
    let doc_ids = store.doc_ids()?;
    let mut doc = store.doc(dkey)?;

    // A `history` opys cannot read is data it must not touch. Refuse *before*
    // any mutation: `run` flushes whether or not this returns an error, so a
    // refusal after `put_doc` would still land the status change on disk without
    // its entry. (`Store::append_history` refuses too, as a backstop.)
    if let Some(why) = crate::history::botched(&doc.frontmatter) {
        return Err(usage(format!(
            "cannot set {id} to {status}: {why}; repair the lifecycle map by hand \
             (see docs/migration-history.md), then retry"
        )));
    }

    // The status graph (ADR-0099 decision 1). A type with no `[transitions]`
    // keeps the any-to-any rule byte-identically; one that declares them accepts
    // only a declared edge. Setting a document to the status it already holds is
    // *not* an edge and never was — it is a legal no-op that appends a history
    // entry — so it is skipped rather than refused for want of a self-edge.
    let current = doc.frontmatter.status().unwrap_or_default().to_string();
    if !current.is_empty() && current != status && !crate::lifecycle::allows(t, &current, status) {
        return Err(usage(format!(
            "cannot move {id} from {current} to {status}: type '{tname}' declares no such \
             transition ({})",
            crate::lifecycle::set_status_targets_phrase(t, &current)
        )));
    }

    // The visit budget (ADR-0101 decision 1), after the graph so a move the type
    // does not declare at all keeps the clearer refusal. `limit = N` allows N
    // entries into the status: a document that already has N gets no more, and
    // the message names the `then` the config chose for exactly this moment.
    // Unlike the graph check this does *not* skip the equal case — a no-op
    // `set-status` appends an entry, and an entry is a visit. There is no
    // bypass: `verify --fix` runs its moves through here like anything else, so
    // the count a document can reach is `limit` and nothing opys writes can
    // carry it past (see [`crate::observe::apply_fixes`]).
    if let Some(mv) = t.max_visits.get(status) {
        let n = crate::observe::visits(&doc, status);
        if mv.limit >= 1 && n >= mv.limit {
            return Err(usage(format!(
                "cannot set {id} to {status}: type '{tname}' allows {} visit(s) to {status} \
                 and this document has had {n} — max_visits sends it to {} instead",
                mv.limit, mv.then
            )));
        }
    }

    if let Some(r) = reason {
        // `--reason` writes `<status>_reason`, which must be a declared field for
        // the type or the very next `verify` rejects it as unknown frontmatter.
        // Guard here so a successful command never leaves a red corpus.
        let field = format!("{status}_reason");
        if !t.fields.contains_key(&field) {
            return Err(usage(format!(
                "--reason has nowhere to go: '{field}' is not a declared field for type \
                 '{tname}' (add [types.{tname}.fields.{field}], or drop --reason)"
            )));
        }
        doc.frontmatter.set_str(&field, r);
    }
    let now = crate::commands::now_utc_micros();
    let prev_status = doc.frontmatter.status().unwrap_or(status).to_string();
    doc.frontmatter.set_str("status", status);
    touch(&mut doc.frontmatter);
    // A document that predates `history` is seeded here — from `created`, at the
    // status it is *leaving* — so the append below lands as its second entry.
    // Appending straight onto it would key the *first* entry to the wall clock,
    // contradicting `created` (verify check 4) and leaving a corpus that verify
    // rejects and the migration procedure can no longer find: it greps for
    // `missing history`, which such a document no longer reports.
    crate::history::seed(&mut doc.frontmatter, &prev_status, &now);

    let problems = rules::evaluate(pcfg, &tname, status, &doc.frontmatter, &doc.body, &doc_ids);
    if !problems.is_empty() {
        return Err(usage(format!(
            "cannot set {id} to {status}: {}",
            problems.join("; ")
        )));
    }

    store.put_doc(pcfg, Some(dkey), &doc)?;
    // After `put_doc`, which rewrites the doc's child rows from `doc` (and would
    // otherwise discard the entry).
    store.append_history(dkey, &now, status)?;
    store.set_canonical_path(pcfg, dkey)?;
    Ok(())
}

pub fn run(ctx: &Ctx, ids: &[String], status: &str, reason: Option<&str>) -> Result<()> {
    let prj = ctx.open()?;
    let ids = resolve_ids(ids)?;
    let (mut store, _) = ctx.load(&prj)?;
    let lines = apply_batch(&ids, |id| {
        core(&prj, &mut store, id, status, reason)?;
        Ok(format!("{id} -> {status}"))
    })?;
    ctx.flush(&prj, store)?;
    print_lines(lines)?;
    maybe_sync(ctx, &prj);
    Ok(())
}
