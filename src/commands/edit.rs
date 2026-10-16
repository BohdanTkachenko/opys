//! Free-form document edits, gated on `verify` not getting worse.
//!
//! These are the writes behind the web UI's edit-in-place (the node's
//! `edit-body` and `set-field`/`remove-field` actions): the body is swapped
//! for the text the caller sent, or one custom frontmatter field is set or
//! removed, and the result is kept **only if it introduces no new verify
//! problems**. New problems, not all problems: a corpus with pre-existing
//! findings can still be edited, it just cannot be made worse. This module is
//! the gate's one implementation — `opys query` is read-only and every other
//! write is a lifecycle command, guarded by the rules engine instead.
//!
//! The gate has a CLI caller too: `opys apply --field k=v` and
//! `--remove-field k` are field writes inside a composite batch, and they go
//! through this gate rather than growing a second, weaker declared-field
//! check. `apply` takes the pair apart, though: it calls [`field_write`] per
//! field and brackets the *whole* batch with one [`problems`] snapshot and one
//! [`refuse_new_problems`], so every step is judged against the finished
//! document rather than against a half-applied one — and the corpus is
//! verified twice per invocation instead of twice per field.
//! [`body_core`] deliberately has none — in a terminal, editing a body *is*
//! editing the file. Either way the core lives in the engine rather than in the
//! server, so that the invariant ("a body write is verify-gated") has exactly
//! one implementation for every consumer.

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
    let before = problems(prj, store, parse_errors)?;
    field_write(prj, store, id, key, value)?;
    refuse_new_problems(prj, store, &before, parse_errors)
}

/// The write half of [`field_core`], with the verify gate left to the caller.
///
/// `apply` brackets a whole composite batch with a single [`problems`] /
/// [`refuse_new_problems`] pair, so it needs the write without a gate around
/// it: a claim's `--field` is legal at the status the same invocation is moving
/// *to*, and gating it on its own would judge it at the status being left. The
/// refusals that are *not* verify findings — a nameless key, a key another path
/// owns, removing a field that is not there — live here, so every caller makes
/// them whether or not it brings its own gate.
pub(crate) fn field_write(
    prj: &Project,
    store: &mut Store,
    id: &str,
    key: &str,
    value: Option<&str>,
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
    Ok(())
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
        "history" => {
            Some("it is auto-maintained lifecycle history written by new/set-status/apply/import")
        }
        k if crate::refs::RELATION_FIELDS.contains(&k) => {
            Some("relation maps are written by block/unblock and the reconcile pass")
        }
        _ => None,
    }
}

/// The shared gate: refuse (leaving the flush to never happen) if the store's
/// verify findings grew relative to `before`.
pub(crate) fn refuse_new_problems(
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
/// Keyed by the problem strings themselves, which is sound because the two
/// snapshots describe the same paths. Most findings are prefixed by the
/// document's id, but several are prefixed by its path instead (a missing or
/// malformed id, an unrecognized prefix, a filename that does not match the id,
/// a duplicate id, a retired id) — and the path a snapshot reports cannot move
/// between the two: `Store::put_doc` re-derives it from the `Doc` the caller
/// read *before* mutating it, and relocating a file to its canonical layout
/// path happens later still, in the flush. That is what lets `apply` bracket a
/// whole batch, status transition included: the transition changes where the
/// document will eventually land, not the path either snapshot prints.
pub(crate) fn problems(
    prj: &Project,
    store: &mut Store,
    parse_errors: &[String],
) -> Result<HashSet<String>> {
    let docs: Vec<_> = store.all_docs()?.into_iter().map(|(_, d)| d).collect();
    Ok(verify::collect_problems(prj, &docs, parse_errors.to_vec())
        .into_iter()
        .collect())
}
