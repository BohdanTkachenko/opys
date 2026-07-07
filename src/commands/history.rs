//! `opys history <id>` — reconstruct a document's lifecycle from git, in-process.
//!
//! Walks the repository with `gix` (no subprocess) and decodes each revision's
//! blob through the real [`Doc`] parser — so the status timeline is read from
//! typed frontmatter, never scraped out of strings.
//!
//! Relocations are handled without fuzzy rename detection: an opys document's
//! filename *is* its ID and never changes when a status change moves the file
//! between directories (e.g. into `_archived/`). So we find the ID-named blob
//! within the inventory base subtree of each commit — exact, threshold-free, and
//! oblivious to where under the base opys put the file. Each content-distinct
//! revision is attributed to the commit that *introduced* it (its blob differs
//! from its first parent's), not the newest commit while it was still current.
//! The whole module is gated behind the optional `history` feature, so the
//! default build has no git dependency.

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
    let (docs, _) = ctx.backend.load_docs(&prj);
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

    // The inventory base, relative to the repo's working tree — we search only
    // this subtree, so a same-named file elsewhere can't shadow the document and
    // we don't walk the entire tree of every commit.
    let workdir = repo.workdir().unwrap_or(prj.root.as_path());
    let base_rel = prj.base.strip_prefix(workdir).unwrap_or(prj.base.as_path());
    let base_components: Vec<String> = base_rel
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str().map(str::to_string),
            _ => None,
        })
        .collect();

    let revs = collect(&repo, &base_components, basename.as_bytes().as_bstr(), id)
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
/// revision (newest first). A revision is attributed to the commit that
/// *introduced* it — the one whose blob differs from its first parent's — so
/// unrelated later commits don't steal the credit. The document is located by
/// its stable ID-named blob within the inventory base subtree, so relocations
/// are followed without rename heuristics.
fn collect(
    repo: &gix::Repository,
    base: &[String],
    basename: &BStr,
    id: &str,
) -> anyhow::Result<Vec<Rev>> {
    let mut revs = Vec::new();
    let mut commit = repo.head_commit()?;
    let mut cur = find_blob(repo, &commit.tree()?, base, basename)?;

    loop {
        // The first parent (linear history; merges follow the mainline) and the
        // blob as it stood there — that is what "current" becomes next iteration.
        let parent = match commit.parent_ids().next() {
            Some(pid) => Some(repo.find_commit(pid.detach())?),
            None => None,
        };
        let parent_oid = match &parent {
            Some(p) => find_blob(repo, &p.tree()?, base, basename)?,
            None => None,
        };

        // This commit introduced the current blob iff its parent had a different
        // (or no) blob — that is the commit the revision belongs to.
        if let Some(oid) = &cur {
            if parent_oid.as_ref() != Some(oid) {
                let blob = repo.find_object(*oid)?.into_blob();
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

        match parent {
            Some(p) => {
                commit = p;
                cur = parent_oid;
            }
            None => break,
        }
    }
    Ok(revs)
}

/// Find the blob named `basename` within the inventory `base` subtree of `tree`,
/// returning its object id. Descends the `base` path components first, so the
/// search is confined to the inventory and a same-named file elsewhere in the
/// repo cannot shadow it. An opys ID is globally unique, so at most one matches.
fn find_blob(
    repo: &gix::Repository,
    tree: &gix::Tree,
    base: &[String],
    basename: &BStr,
) -> anyhow::Result<Option<gix::ObjectId>> {
    // Descend into the base directory; if any component is absent in this commit
    // (the inventory didn't exist yet), there is nothing to find.
    let mut scope = tree.clone();
    for comp in base {
        let Some(entry) = scope.iter().find_map(|e| {
            let e = e.ok()?;
            (e.mode().is_tree() && e.filename().to_str().ok() == Some(comp.as_str())).then_some(e)
        }) else {
            return Ok(None);
        };
        scope = repo.find_tree(entry.oid().to_owned())?;
    }
    search_tree(repo, &scope, basename)
}

/// Recursively search `tree` for a blob named `basename`.
fn search_tree(
    repo: &gix::Repository,
    tree: &gix::Tree,
    basename: &BStr,
) -> anyhow::Result<Option<gix::ObjectId>> {
    for entry in tree.iter() {
        let entry = entry?;
        if entry.mode().is_tree() {
            let sub = repo.find_tree(entry.oid().to_owned())?;
            if let Some(found) = search_tree(repo, &sub, basename)? {
                return Ok(Some(found));
            }
        } else if entry.filename() == basename {
            return Ok(Some(entry.oid().to_owned()));
        }
    }
    Ok(None)
}
