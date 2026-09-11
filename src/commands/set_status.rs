//! `opys set-status` — a guarded status transition through the corpus store.
//!
//! The mutation pattern shared by every scalar mutator: reconstruct the doc,
//! apply the change with the existing frontmatter/`touch` helpers, validate
//! (rules engine) on that transient clone, and only then write it back via
//! `Store::put_doc` (which re-decomposes into the tables). Guards run before
//! any write, so a failed id leaves the store untouched.
//!
//! The guards themselves ([`check_move`]) are shared with the composite write
//! [`crate::commands::apply`], which differs in exactly one of them — see
//! [`Move`] — so the two commands cannot drift apart on what a legal status
//! change is.

use crate::commands::{apply_batch, maybe_sync, print_lines, resolve_ids, touch};
use crate::error::{usage, Result};
use crate::project::Project;
use crate::project_config::DocType;
use crate::rules;
use crate::store::Store;
use crate::Ctx;

/// What a status write asserts about where the document currently stands.
///
/// The two differ in exactly one place: the equal case. `set-status` has always
/// treated "move it to the status it already holds" as a legal no-op that
/// appends a history entry — [`crate::lifecycle::allows`] is never even
/// consulted for it — and that must not change. A composite write
/// ([`crate::commands::apply`]) means something stronger: it *asserts a
/// transition*, and that assertion is what makes a claim race decidable. The
/// second claimant finds the document already in the working status and is
/// refused, instead of quietly overwriting the winner's claim.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Move {
    /// `opys set-status`: the equal case is a no-op that still appends an entry.
    Set,
    /// `opys apply --status`: the equal case is a refusal.
    Transition,
}

/// The structural guards on a status write — unrecognized id prefix, unknown
/// status, a terminal target, and the status graph — run against the store
/// **without mutating anything**.
///
/// Split out of [`core_with`] so a composite write can be refused before it has
/// written a field or a log entry: the loser of a claim race must get the
/// transition guard's answer, not whatever the first part of its batch happened
/// to hit. `core_with` re-runs these itself, so this is a cheap, unambiguous
/// pre-flight rather than the enforcement point.
pub fn check_move(
    prj: &Project,
    store: &mut Store,
    id: &str,
    status: &str,
    mode: Move,
) -> Result<()> {
    let (tname, t) = resolve_type(prj, id)?;
    check_status(t, &tname, id, status)?;
    let dkey = store.dkey_of(id)?;
    let current = store.doc(dkey)?;
    let current = current.frontmatter.status().unwrap_or_default().to_string();
    check_graph(t, &tname, id, &current, status, mode)
}

/// The document type of `id`, or a usage error naming the unrecognized prefix.
fn resolve_type<'a>(prj: &'a Project, id: &str) -> Result<(String, &'a DocType)> {
    let tname = prj
        .pcfg
        .type_name_for_id(id)
        .ok_or_else(|| usage(format!("unrecognized id prefix in {id}")))?
        .to_string();
    let t = &prj.pcfg.types[&tname];
    Ok((tname, t))
}

/// `status` is one of the type's, and is not terminal — terminal statuses are
/// `close`'s door alone.
fn check_status(t: &DocType, tname: &str, id: &str, status: &str) -> Result<()> {
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
    Ok(())
}

/// The status graph (ADR-0099 decision 1), plus the equal-case rule `mode`
/// selects.
///
/// A type with no `[transitions]` keeps the any-to-any rule byte-identically.
/// The equal case is answered *before* [`crate::lifecycle::allows`] is
/// consulted, and by `mode` rather than by the map: under [`Move::Set`] it is
/// the legal no-op it has always been, and under [`Move::Transition`] it is
/// refused whether or not the type declares a graph — so a composite write
/// settles a claim race for every type, not only for one whose author wrote a
/// map.
fn check_graph(
    t: &DocType,
    tname: &str,
    id: &str,
    current: &str,
    status: &str,
    mode: Move,
) -> Result<()> {
    if current.is_empty() {
        return Ok(());
    }
    if current == status {
        return match mode {
            Move::Set => Ok(()),
            Move::Transition => Err(usage(format!(
                "cannot move {id} from {current} to {status}: it is already {status}, and a \
                 composite write asserts a transition — another writer got there first"
            ))),
        };
    }
    if !crate::lifecycle::allows(t, current, status) {
        return Err(usage(format!(
            "cannot move {id} from {current} to {status}: type '{tname}' declares no such \
             transition ({})",
            crate::lifecycle::set_status_targets_phrase(t, current)
        )));
    }
    Ok(())
}

/// Apply a status transition to the store. Does not flush/print/sync.
pub fn core(
    prj: &Project,
    store: &mut Store,
    id: &str,
    status: &str,
    reason: Option<&str>,
) -> Result<()> {
    core_with(prj, store, id, status, reason, Move::Set)
}

/// [`core`] with the equal-case rule chosen by the caller: `set-status` passes
/// [`Move::Set`], the composite write passes [`Move::Transition`].
pub fn core_with(
    prj: &Project,
    store: &mut Store,
    id: &str,
    status: &str,
    reason: Option<&str>,
    mode: Move,
) -> Result<()> {
    let pcfg = &prj.pcfg;
    let (tname, t) = resolve_type(prj, id)?;
    check_status(t, &tname, id, status)?;

    let dkey = store.dkey_of(id)?;
    let doc_ids = store.doc_ids()?;
    let mut doc = store.doc(dkey)?;

    // A `history` opys cannot read is data it must not touch. Refuse *before*
    // any mutation, so a partly-applied change cannot be left in the store for
    // some later caller to flush. (`Store::append_history` refuses too, as a
    // backstop.)
    if let Some(why) = crate::history::botched(&doc.frontmatter) {
        return Err(usage(format!(
            "cannot set {id} to {status}: {why}; repair the lifecycle map by hand \
             (see docs/migration-history.md), then retry"
        )));
    }

    // The status graph (ADR-0099 decision 1) and the equal-case rule `mode`
    // selects — see [`check_graph`].
    let current = doc.frontmatter.status().unwrap_or_default().to_string();
    check_graph(t, &tname, id, &current, status, mode)?;

    // The visit budget (ADR-0101 decision 1), after the graph so a move the type
    // does not declare at all keeps the clearer refusal. `limit = N` allows N
    // entries into the status: a document that already has N gets no more, and
    // the message names the `then` the config chose for exactly this moment.
    // Unlike the graph check this does *not* skip the equal case — a no-op
    // `set-status` appends an entry, and an entry is a visit. There is no
    // bypass, and no caller has one: this is the only path that *appends a
    // visit* to a document that already exists. `new` and `import` seed exactly
    // one entry as they create it, and `history --seed` reconstructs a past that
    // already happened (refusing any document that carries entries at all). So
    // the count an opys transition can carry a document to is `limit` and no
    // further (which is what makes [`crate::observe::overflow`]'s `>= limit`
    // reading reachable rather than a dead letter).
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
