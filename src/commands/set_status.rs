//! `opys set-status` — a guarded status transition through the corpus store.
//!
//! The mutation pattern shared by every scalar mutator: reconstruct the doc,
//! apply the change with the existing frontmatter/`touch` helpers, validate
//! (rules engine) on that transient clone, and only then write it back via
//! `Store::put_doc` (which re-decomposes into the tables). Guards run before
//! any write, so a failed id leaves the store untouched.

use crate::commands::{expand_ids, for_each_id, maybe_sync, touch};
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
    doc.frontmatter.set_str("status", status);
    touch(&mut doc.frontmatter);

    let problems = rules::evaluate(pcfg, &tname, status, &doc.frontmatter, &doc.body, &doc_ids);
    if !problems.is_empty() {
        return Err(usage(format!(
            "cannot set {id} to {status}: {}",
            problems.join("; ")
        )));
    }

    store.put_doc(pcfg, Some(dkey), &doc)?;
    store.set_canonical_path(pcfg, dkey)?;
    Ok(())
}

pub fn run(ctx: &Ctx, ids: &str, status: &str, reason: Option<&str>) -> Result<()> {
    let prj = ctx.open()?;
    let ids = expand_ids(ids)?;
    let (mut store, _) = ctx.load(&prj)?;
    let res = for_each_id(&ids, |id| {
        core(&prj, &mut store, id, status, reason)?;
        println!("{id} -> {status}");
        Ok(())
    });
    ctx.flush(&prj, store)?;
    maybe_sync(ctx, &prj);
    res
}
