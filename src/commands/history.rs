//! `opys history <id>` — reconstruct a document's lifecycle from git, in-process.
//!
//! Walks the repository with `gix` (no subprocess) and decodes each revision's
//! blob through the real [`Doc`] parser — so the status timeline is read from
//! typed frontmatter, never scraped out of strings.
//!
//! Relocations are handled without fuzzy rename detection: an opys document's
//! filename *is* its ID and never changes when a status change moves the file
//! between directories (e.g. into `_archived/`). So we simply find the
//! ID-named blob anywhere in each commit's tree — exact, threshold-free, and
//! oblivious to where opys chose to put the file. The whole module is gated
//! behind the optional `history` feature, so the default build has no git
//! dependency.

use std::path::PathBuf;

use gix::bstr::{BStr, ByteSlice};

use crate::doc::Doc;
use crate::error::{usage, Result};
use crate::Ctx;

/// One content-distinct revision of the document.
struct Rev {
    short: String,
    date: String,
    author: String,
    status: String,
    summary: String,
}

pub fn run(ctx: &Ctx, id: &str) -> Result<()> {
    let prj = ctx.open()?;
    let (docs, _) = prj.load_docs();
    let doc = prj.find(&docs, id)?;

    // The document's filename is its ID and is stable across relocations; that
    // basename is all we need to track it through history.
    let basename = doc
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| usage("git history: document has no file name"))?
        .to_owned();

    // Discover the repository from the project root (it may sit above it).
    let repo = gix::discover(&prj.root).map_err(|e| usage(format!("git history: {e}")))?;
    let revs = collect(&repo, basename.as_bytes().as_bstr(), id)
        .map_err(|e| usage(format!("git history: {e}")))?;

    if revs.is_empty() {
        println!("{id}: no committed history found");
        return Ok(());
    }

    let author_w = revs.iter().map(|r| r.author.len()).max().unwrap_or(0);
    println!(
        "History of {id} ({} revision{}, newest first):",
        revs.len(),
        if revs.len() == 1 { "" } else { "s" }
    );
    for r in &revs {
        println!(
            "  {}  {}  {:<aw$}  {:<14}  {}",
            r.short,
            r.date,
            r.author,
            r.status,
            r.summary,
            aw = author_w
        );
    }
    Ok(())
}

/// Walk first-parent from HEAD and return one [`Rev`] per content-distinct
/// revision (newest first), locating the document by its stable ID-named blob in
/// each commit's tree so relocations are followed without rename heuristics.
fn collect(repo: &gix::Repository, basename: &BStr, id: &str) -> anyhow::Result<Vec<Rev>> {
    let mut revs = Vec::new();
    let mut commit = repo.head_commit()?;
    let mut last_oid: Option<gix::ObjectId> = None;

    loop {
        let tree = commit.tree()?;
        if let Some(oid) = find_blob(repo, &tree, basename)? {
            // Emit only when the blob content actually changed.
            if last_oid.as_ref() != Some(&oid) {
                last_oid = Some(oid);
                let blob = repo.find_object(oid)?.into_blob();
                let text = String::from_utf8_lossy(&blob.data);
                // Decode through the canonical parser; fall back gracefully if a
                // historical revision predates the current frontmatter shape.
                let status = Doc::parse(PathBuf::from(format!("{id}.md")), &text)
                    .ok()
                    .and_then(|d| d.status().map(str::to_string))
                    .unwrap_or_else(|| "-".into());
                revs.push(Rev {
                    short: commit.id().to_hex_with_len(8).to_string(),
                    date: commit.time()?.format(gix::date::time::format::SHORT)?,
                    author: commit.author()?.name.to_string(),
                    status,
                    summary: commit.message()?.summary().to_string(),
                });
            }
        }

        // Step to the first parent (linear history; merges follow mainline).
        let parents: Vec<_> = commit.parent_ids().collect();
        let Some(parent_id) = parents.first().copied() else {
            break;
        };
        commit = repo.find_commit(parent_id)?;
    }
    Ok(revs)
}

/// Find the blob named `basename` anywhere in `tree`, returning its object id.
/// An opys ID is globally unique, so at most one such file exists per commit.
fn find_blob(
    repo: &gix::Repository,
    tree: &gix::Tree,
    basename: &BStr,
) -> anyhow::Result<Option<gix::ObjectId>> {
    for entry in tree.iter() {
        let entry = entry?;
        if entry.mode().is_tree() {
            let sub = repo.find_tree(entry.oid().to_owned())?;
            if let Some(found) = find_blob(repo, &sub, basename)? {
                return Ok(Some(found));
            }
        } else if entry.filename() == basename {
            return Ok(Some(entry.oid().to_owned()));
        }
    }
    Ok(None)
}
