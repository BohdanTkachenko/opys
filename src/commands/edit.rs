//! Free-form document edits, gated on `verify` not getting worse.
//!
//! These are the writes behind the web UI's edit-in-place (the node's
//! `edit-body` and `set-field`/`remove-field` actions): the body is swapped
//! for the text the caller sent, or one custom frontmatter field is set or
//! removed, and the result is kept **only if it introduces no new verify
//! problems** — the same contract `opys query --write` gives an arbitrary SQL
//! edit. New problems, not all problems: a corpus with pre-existing findings
//! can still be edited, it just cannot be made worse.
//!
//! There is deliberately no CLI subcommand mounted on this yet: in a terminal,
//! editing a body *is* editing the file. The core lives in the engine rather
//! than in the server so that the invariant ("a body write is verify-gated")
//! has exactly one implementation when a CLI or another consumer wants it.

use std::collections::HashSet;

use crate::commands::{now_rfc3339, verify};
use crate::error::{usage, Result};
use crate::project::Project;
use crate::store::Store;

/// Replace `id`'s body with `body` inside the loaded store.
///
/// The store mutation stays in memory: the caller flushes on `Ok` and simply
/// drops the store on `Err`, so a refused edit leaves no trace on disk.
/// `parse_errors` are the load's unparsable-document messages — they are part
/// of the verify baseline, exactly as `verify` itself would report them.
pub fn body_core(
    prj: &Project,
    store: &mut Store,
    id: &str,
    body: &str,
    parse_errors: &[String],
) -> Result<()> {
    let dkey = store.dkey_of(id)?;
    let before = problems(prj, store, parse_errors)?;

    let mut doc = store.doc(dkey)?;
    // Canonical shape: exactly one trailing newline, like every body the
    // engine itself writes. Without this, an editor that strips (or doubles)
    // the final newline would dirty every future diff of the file.
    let mut text = body.trim_end_matches('\n').to_string();
    text.push('\n');
    doc.body = text;
    doc.title = crate::body::title(&doc.body);
    store.put_doc(&prj.pcfg, Some(dkey), &doc)?;
    store.touch(dkey, &now_rfc3339())?;

    refuse_new_problems(prj, store, &before, parse_errors)
}

/// Set — or with `value` `None`, remove — one custom frontmatter field, gated
/// exactly as [`body_core`] gates a body: the write lands only if it
/// introduces no new verify problems, so the closed-frontmatter invariant, the
/// declared field types, and the enum/pattern constraints are all enforced by
/// the one engine that owns them, with their own messages.
///
/// The value string is read by [`crate::project::parse_field_value`] — the
/// same coercion as the CLI's `--field key=value` — so `3` is an int, `[a, b]`
/// a list, and quoting forces a string.
///
/// Keys that a dedicated path owns are refused up front rather than through
/// the gate, because the gate could not catch them: a raw `status` write would
/// pass verify while skipping the write-time rules `set-status` runs, and an
/// `updated` write would pass and then be silently clobbered by the very
/// timestamp bump this write performs.
pub fn field_core(
    prj: &Project,
    store: &mut Store,
    id: &str,
    key: &str,
    value: Option<&str>,
    parse_errors: &[String],
) -> Result<()> {
    let key = key.trim();
    if key.is_empty() {
        return Err(usage("a field needs a name"));
    }
    if let Some(owner) = owned_elsewhere(key) {
        return Err(usage(format!(
            "'{key}' is not editable as a field — {owner}"
        )));
    }
    let dkey = store.dkey_of(id)?;
    let before = problems(prj, store, parse_errors)?;

    let mut doc = store.doc(dkey)?;
    match value {
        Some(v) => {
            doc.frontmatter
                .insert(key, crate::project::parse_field_value(v));
        }
        None => {
            if doc.frontmatter.remove(key).is_none() {
                return Err(usage(format!("{id} has no field '{key}' to remove")));
            }
        }
    }
    store.put_doc(&prj.pcfg, Some(dkey), &doc)?;
    store.touch(dkey, &now_rfc3339())?;

    refuse_new_problems(prj, store, &before, parse_errors)
}

/// The dedicated write path that owns `key`, if one does. `created` is
/// deliberately absent: it is auto-*seeded*, not auto-maintained — nothing
/// rewrites it after the fact, so correcting one is an ordinary edit (verify
/// still holds it to RFC3339).
fn owned_elsewhere(key: &str) -> Option<&'static str> {
    match key {
        "id" => Some("ids are permanent (see `opys renumber`)"),
        "status" => Some("use set-status, which runs the status rules"),
        "tags" => Some("use tag"),
        "updated" => Some("it is auto-maintained; every write refreshes it"),
        "blocked_from" => Some("it is auto-maintained bookkeeping for block/unblock"),
        k if crate::refs::RELATION_FIELDS.contains(&k) => {
            Some("relation maps are written by block/unblock and the reconcile pass")
        }
        _ => None,
    }
}

/// The shared gate: refuse (leaving the flush to never happen) if the store's
/// verify findings grew relative to `before`.
fn refuse_new_problems(
    prj: &Project,
    store: &mut Store,
    before: &HashSet<String>,
    parse_errors: &[String],
) -> Result<()> {
    let after = problems(prj, store, parse_errors)?;
    let new: Vec<String> = after.difference(before).cloned().collect();
    if !new.is_empty() {
        return Err(usage(format!(
            "refusing the edit — it would introduce {} verify problem(s):\n  {}",
            new.len(),
            new.join("\n  ")
        )));
    }
    Ok(())
}

/// The corpus's verify findings as a set, for a before/after difference.
///
/// Keyed by the problem strings themselves. A body edit never moves a file, so
/// unlike `query --write` (whose SQL can rename and relocate) the strings are
/// stable enough to difference directly.
fn problems(prj: &Project, store: &mut Store, parse_errors: &[String]) -> Result<HashSet<String>> {
    let docs: Vec<_> = store.all_docs()?.into_iter().map(|(_, d)| d).collect();
    Ok(verify::collect_problems(prj, &docs, parse_errors.to_vec())
        .into_iter()
        .collect())
}
