//! `opys history <id>` — reconstruct a document's lifecycle from git, and
//! `--seed`, the one-off migration that turns that timeline into the document's
//! frontmatter lifecycle map.
//!
//! Both read the repository through [`crate::git`]: in-process with `gix`, no
//! subprocess, and every revision decoded through the real [`Doc`] parser, so
//! the status timeline comes from typed frontmatter rather than being scraped
//! out of strings.
//!
//! The reading half is gated behind the `git` feature (on by default). The
//! *command* is not: a build without the feature still parses `opys history`
//! and answers it by naming the feature it lacks ([`run`] below), because clap's
//! "unrecognized subcommand 'history'; a similar subcommand exists: 'list'" is
//! not an answer to what the user asked. Unlike the reader, it does not degrade
//! to an empty timeline — that would claim it looked.

#[cfg(not(feature = "git"))]
use crate::error::usage;
use crate::error::Result;
use crate::Ctx;
#[cfg(feature = "git")]
use {
    crate::commands::{maybe_sync, now_utc_micros, print_lines},
    crate::error::usage,
    crate::git,
    crate::history,
};

/// Main-only fallback: this binary has no git reader, and says so rather than
/// pretending the timeline is empty. Exit 2, like any other usage error.
#[cfg(not(feature = "git"))]
pub fn run(_ctx: &Ctx, _id: &str, _seed: bool) -> Result<()> {
    Err(usage(crate::git::NO_GIT_FEATURE_COMMAND))
}

#[cfg(feature = "git")]
pub fn run(ctx: &Ctx, id: &str, seed: bool) -> Result<()> {
    if seed {
        seed_from_git(ctx, id)
    } else {
        show(ctx, id)
    }
}

/// Print the status timeline, newest first.
#[cfg(feature = "git")]
fn show(ctx: &Ctx, id: &str) -> Result<()> {
    let prj = ctx.open()?;
    let (docs, _) = ctx.backend.load_docs(&prj);
    let doc = prj.find(&docs, id)?;
    let basename = basename_of(&doc.path, id)?;

    let repo = open(&prj)?;
    let revs = git::timeline(&repo, &prj, &basename, id)
        .map_err(|e| usage(format!("git history: {e}")))?;

    if revs.is_empty() {
        print_lines([format!("{id}: no committed history found")])?;
        return Ok(());
    }

    let author_w = revs.iter().map(|r| r.author.len()).max().unwrap_or(0);
    let mut lines = vec![format!(
        "History of {id} ({} revision{}, newest first):",
        revs.len(),
        if revs.len() == 1 { "" } else { "s" }
    )];
    lines.extend(revs.iter().map(|r| {
        format!(
            "  {}  {}  {:<aw$}  {:<14}  {}",
            r.short,
            r.date,
            r.author,
            r.status,
            r.summary,
            aw = author_w
        )
    }));
    // A merge commit's first-parent git history omits commits on the work
    // branch. The lifecycle map is carried by the merged document itself, so
    // show it separately rather than inventing commit metadata for its stages.
    if prj
        .pcfg
        .type_name_for_id(id)
        .is_some_and(|n| !prj.pcfg.types[n].work_branches.is_empty())
    {
        let entries = crate::history::entries(&doc.frontmatter);
        if !entries.is_empty() {
            lines.push("Recorded lifecycle (this copy, newest first):".to_string());
            lines.extend(
                entries
                    .iter()
                    .rev()
                    .map(|(since, status)| format!("  {since}  {status}")),
            );
        }
    }
    print_lines(lines)?;
    Ok(())
}

/// Write the document's lifecycle map from its git timeline: one entry per
/// status the document has held, keyed by the commit that introduced it.
///
/// Three rules keep a successful seed from leaving a corpus `verify` rejects,
/// and they are the reason this is a command rather than a script:
///
/// 1. the **first** entry is keyed from `created` when the document has one
///    (that is what [`history::seed`] does), not from the first commit. `verify`
///    requires `created` and the first key to denote the same second, and a
///    document is essentially never committed in the second it was created;
/// 2. later entries go through [`history::append`], which bumps a key that is
///    not strictly greater than the previous one by a microsecond — so a
///    timeline whose commits predate `created`, or two transitions in one
///    second, still produces strictly increasing keys;
/// 3. when the working copy's `status` is ahead of the last committed one (the
///    normal state mid-migration, because the change that prompted it is not
///    committed yet), a final entry at the wall clock records it. `verify`
///    requires the last entry to match `status`.
///
/// It refuses to overwrite anything carrying entries. History cannot be rebuilt
/// from ground truth, so a map that is already there is data — the same promise
/// every other write path makes.
#[cfg(feature = "git")]
fn seed_from_git(ctx: &Ctx, id: &str) -> Result<()> {
    let prj = ctx.open()?;
    let (mut store, _) = ctx.load(&prj)?;
    let message = seed_in_store(&prj, &mut store, id)?;
    ctx.flush(&prj, store)?;
    print_lines([message])?;
    maybe_sync(ctx, &prj);
    Ok(())
}

/// Shared by the explicit migration and `verify --fix`, under the caller's lock.
#[cfg(feature = "git")]
fn seed_in_store(
    prj: &crate::Project,
    store: &mut crate::store::Store,
    id: &str,
) -> Result<String> {
    let dkey = store.dkey_of(id)?;
    let mut doc = store.doc(dkey)?;

    // Refuse *before* any mutation and before the repository is even opened: a
    // refusal that had already written something would be the one thing this
    // command must never do.
    if let Some(why) = history::botched(&doc.frontmatter) {
        return Err(usage(format!(
            "cannot seed {id}: {why}; repair the lifecycle map by hand, \
             then retry"
        )));
    }
    let existing = history::entries(&doc.frontmatter).len();
    if existing > 0 {
        return Err(usage(format!(
            "{id} already has a lifecycle map ({existing} entr{}); --seed refuses to \
             overwrite it — history is not rebuildable, so an existing map is data",
            if existing == 1 { "y" } else { "ies" }
        )));
    }

    let basename = basename_of(&doc.path, id)?;
    let repo = open(prj)?;
    let revs =
        git::timeline(&repo, prj, &basename, id).map_err(|e| usage(format!("git history: {e}")))?;

    // Oldest first; drop revisions no status could be read out of (a document
    // that predates the tool's frontmatter, say — git can only show what was
    // committed), and collapse runs of the same status, keeping the commit that
    // introduced it.
    let mut timeline: Vec<(i64, String)> = revs
        .iter()
        .rev()
        .filter(|r| r.status != "-")
        .map(|r| (r.secs, r.status.clone()))
        .collect();
    timeline.dedup_by(|a, b| a.1 == b.1);
    if timeline.is_empty() {
        return Err(usage(format!(
            "{id}: git has no committed revision carrying a status — nothing to seed \
             from; add one entry by hand, keyed by `created` in UTC, at its current status"
        )));
    }

    let (first_secs, first_status) = &timeline[0];
    history::seed(&mut doc.frontmatter, first_status, &key_at(*first_secs)?);
    for (secs, status) in &timeline[1..] {
        history::append(&mut doc.frontmatter, &key_at(*secs)?, status);
    }
    // The working copy may be ahead of the last commit — that is the ordinary
    // case for the document whose status change prompted the migration.
    let current = doc.frontmatter.status().unwrap_or_default().to_string();
    if !current.is_empty() && timeline.last().map(|(_, s)| s.as_str()) != Some(current.as_str()) {
        history::append(&mut doc.frontmatter, &now_utc_micros(), &current);
    }

    // Deliberately no `touch`: a seed reconstructs the past, it is not an edit,
    // and the migration procedure's round-trip probe promises that `updated` is
    // the only line that may move.
    let entries = history::entries(&doc.frontmatter);
    let summary = match (entries.first(), entries.last()) {
        (Some((first, _)), Some((last, _))) if first != last => format!("{first} .. {last}"),
        (Some((first, _)), _) => first.clone(),
        _ => String::new(),
    };
    let count = entries.len();
    store.put_doc(&prj.pcfg, Some(dkey), &doc)?;
    Ok(format!(
        "{id}: seeded {count} history entr{} from git ({summary})",
        if count == 1 { "y" } else { "ies" }
    ))
}

/// Seed only absent/empty maps. Existing and malformed history is never replaced;
/// failures are findings so one unseedable document cannot stop the whole pass.
pub(super) fn seed_missing(
    prj: &crate::Project,
    store: &mut crate::store::Store,
    only: Option<&str>,
) -> Result<crate::observe::FixReport> {
    let mut report = crate::observe::FixReport::default();
    for (_, doc) in store.all_docs()? {
        if crate::history::botched(&doc.frontmatter).is_some()
            || !crate::history::entries(&doc.frontmatter).is_empty()
        {
            continue;
        }
        let Some(id) = doc.id() else { continue };
        if only.is_some_and(|wanted| wanted != id) {
            continue;
        }
        #[cfg(feature = "git")]
        match seed_in_store(prj, store, id) {
            Ok(message) => report.applied.push(message),
            Err(error) => report
                .refused
                .push(format!("{id}: automatic history seed refused: {error}")),
        }
        #[cfg(not(feature = "git"))]
        {
            let _ = prj;
            report.refused.push(format!(
                "{id}: automatic history seed requires the git feature"
            ));
        }
    }
    Ok(report)
}

/// The document's filename, which is its id and is stable across relocations —
/// what every blob lookup keys on.
#[cfg(feature = "git")]
fn basename_of(path: &std::path::Path, id: &str) -> Result<String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .ok_or_else(|| usage(format!("git history: {id} has no file name")))
}

#[cfg(feature = "git")]
fn open(prj: &crate::Project) -> Result<gix::Repository> {
    git::open(prj)
        .map_err(|e| usage(format!("git history: {e}")))?
        .ok_or_else(|| {
            // `components()` drops the interior `.` that `--root .` leaves in
            // the discovered root, so the message names a path a user can paste.
            let root: std::path::PathBuf = prj.root.components().collect();
            usage(format!(
                "git history: {} is not in a git repository — there is no \
                 committed timeline to read",
                root.display()
            ))
        })
}

/// A commit's instant as a canonical lifecycle key.
#[cfg(feature = "git")]
fn key_at(secs: i64) -> Result<String> {
    time::OffsetDateTime::from_unix_timestamp(secs)
        .map(history::format_key)
        .map_err(|_| usage(format!("git history: commit time {secs} is out of range")))
}
