//! The gix-backed half of [`crate::git`] — every item that mentions a git
//! object. Compiled only with the `git` feature; the module's data types and
//! the main-only fallback live in its parent and are always compiled.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use gix::bstr::{BStr, ByteSlice};
use gix::ObjectId;

use super::{Changed, Occupancy, Occupant, Ref, NOT_A_REPOSITORY, NO_TRUTH_BRANCH};
use crate::doc::Doc;
use crate::project::Project;
use crate::project_config::Git;

/// A document file under the inventory base: `PREFIX-NNNN.md`, the id-named
/// blob every lookup here keys on.
fn doc_id_of(basename: &str) -> Option<&str> {
    let stem = basename.strip_suffix(".md")?;
    let (prefix, num) = stem.split_once('-')?;
    let prefix_ok = !prefix.is_empty()
        && prefix.starts_with(|c: char| c.is_ascii_uppercase())
        && prefix
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    let num_ok = !num.is_empty() && num.chars().all(|c| c.is_ascii_digit());
    (prefix_ok && num_ok).then_some(stem)
}

/// Open the repository the project lives in, or `None` when it lives in none.
///
/// Discovery searches upward from the project root, so a project nested inside a
/// repository is found. Every *discovery* failure is `None` — "no repository
/// here" is the only thing it can mean, and ADR-0102 says that degrades rather
/// than fails; a repository that is found but cannot be opened is a real error.
pub fn open(prj: &Project) -> anyhow::Result<Option<gix::Repository>> {
    match gix::discover(&prj.root) {
        Ok(repo) => Ok(Some(repo)),
        Err(gix::discover::Error::Discover(_)) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// The truth branch and its commit: `[git] main` when `refs/heads/<main>`
/// resolves, else `refs/heads/master`.
///
/// The fallback is not a guess about taste — a repository that never renamed its
/// default branch must work with no `[git]` table at all, and there is no
/// ambiguity: it is consulted only when the configured name resolves to nothing.
pub fn truth_branch(repo: &gix::Repository, cfg: &Git) -> Option<(String, ObjectId)> {
    for name in [cfg.main.as_str(), "master"] {
        let full = format!("refs/heads/{name}");
        if let Ok(Some(mut r)) = repo.try_find_reference(full.as_str()) {
            if let Ok(id) = r.peel_to_id() {
                return Some((full, id.detach()));
            }
        }
    }
    None
}

/// Whether a fully qualified ref name matches one of the `[git] branches`
/// patterns, using git's own wildmatch with `*` crossing `/` — refspec
/// semantics, so `refs/heads/*` covers `refs/heads/feat/x`.
fn selected(patterns: &[String], name: &str) -> bool {
    patterns.iter().any(|p| {
        gix::glob::wildmatch(
            p.as_bytes().as_bstr(),
            name.as_bytes().as_bstr(),
            gix::glob::wildmatch::Mode::empty(),
        )
    })
}

/// What to call a branch in a column header: `refs/heads/x` → `x`,
/// `refs/remotes/origin/x` → `origin/x`; anything else keeps its full name,
/// because a bare `v1.2` that is really a tag would be read as a branch.
fn short_name(full: &str) -> String {
    full.strip_prefix("refs/heads/")
        .or_else(|| full.strip_prefix("refs/remotes/"))
        .unwrap_or(full)
        .to_string()
}

/// The names pointing at one commit, before a label is chosen.
#[derive(Default)]
struct Candidate {
    refs: Vec<String>,
    worktrees: Vec<String>,
}

/// Every ref that holds the inventory, deduplicated by commit.
///
/// The set is the `[git] branches` patterns matched against fully qualified ref
/// names (local heads by default, so remote-tracking refs are opt-in) plus every
/// **linked worktree head**, read straight out of `<git-dir>/worktrees/*/HEAD`
/// because a jj-colocated worktree detaches HEAD and no ref covers it. The truth
/// branch is excluded: it is main, not progress.
///
/// Labelling, where two rules meet: a branch name wins over a directory name, so
/// a detached worktree parked on a branch's commit is one column named for the
/// branch. With no branch, the label is the checkout directory names of the
/// worktrees on that commit, sorted and joined with `", "` — two worktrees on one
/// commit are one column (it *is* one commit) that names both, rather than one of
/// them silently winning.
///
/// The main worktree's own HEAD is deliberately not a ref here: it is the
/// checkout the caller is reading from disk, so reporting it would show every
/// project as occupying itself.
pub fn refs(repo: &gix::Repository, cfg: &Git) -> anyhow::Result<Vec<Ref>> {
    let truth = truth_branch(repo, cfg);
    let truth_name = truth.as_ref().map(|(n, _)| n.clone());
    let truth_commit = truth.as_ref().map(|(_, c)| *c);

    let mut found: BTreeMap<ObjectId, Candidate> = BTreeMap::new();

    for r in repo.references()?.all()? {
        // The iterator's error is a boxed `dyn Error`, which `anyhow` will not
        // take through `?`; the message is all there is to report anyway.
        let mut r = r.map_err(|e| anyhow::anyhow!("{e}"))?;
        let name = r.name().as_bstr().to_string();
        if Some(&name) == truth_name.as_ref() || !selected(&cfg.branches, &name) {
            continue;
        }
        // A ref that peels to nothing (a dangling symbolic ref, a tag object
        // whose target is missing) is skipped rather than failing the read: one
        // broken ref must not blind the caller to every other branch.
        if let Ok(id) = r.peel_to_id() {
            found.entry(id.detach()).or_default().refs.push(name);
        }
    }

    for wt in repo.worktrees()? {
        let Some(id) = worktree_head(repo, wt.git_dir()) else {
            continue;
        };
        found
            .entry(id)
            .or_default()
            .worktrees
            .push(worktree_label(&wt));
    }

    let mut out: Vec<Ref> = found
        .into_iter()
        .filter(|(oid, _)| Some(*oid) != truth_commit)
        .map(|(oid, mut c)| {
            c.refs.sort();
            c.worktrees.sort();
            let label = match c.refs.first() {
                Some(name) => short_name(name),
                None => c.worktrees.join(", "),
            };
            let name = match (c.refs.first(), c.worktrees.first()) {
                (Some(name), _) => name.clone(),
                (None, Some(dir)) => format!("worktrees/{dir}"),
                (None, None) => String::new(),
            };
            Ref {
                name,
                commit: oid.to_hex().to_string(),
                label,
            }
        })
        .collect();
    out.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.name.cmp(&b.name)));
    Ok(out)
}

/// The commit a linked worktree's HEAD names. Both spellings are handled on
/// purpose: plain git writes `ref: refs/heads/x`, a jj-colocated worktree writes
/// a bare object id — and that detached case is the one the union view exists
/// for, so reading only the symbolic form would drop exactly the worktrees that
/// matter.
fn worktree_head(repo: &gix::Repository, git_dir: &Path) -> Option<ObjectId> {
    let text = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let text = text.trim();
    match text.strip_prefix("ref:") {
        Some(name) => {
            let mut r = repo.try_find_reference(name.trim()).ok()??;
            r.peel_to_id().ok().map(|id| id.detach())
        }
        None => ObjectId::from_hex(text.as_bytes()).ok(),
    }
}

/// A worktree's checkout directory name — what a human calls that worktree —
/// falling back to its entry under `worktrees/` when the `gitdir` file cannot be
/// read.
fn worktree_label(wt: &gix::worktree::Proxy<'_>) -> String {
    wt.base()
        .ok()
        .as_deref()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| wt.id().to_string())
}

/// The inventory base as path components relative to the repository's work
/// tree — the subtree every lookup is confined to.
///
/// Both sides are canonicalized before stripping. `prj.base` and `prj.root` are
/// deliberately *un*canonicalized (a `--root` the user typed, joined with the
/// configured base) while gix canonicalizes its work directory, so a symlinked
/// temp dir made the naive `base.strip_prefix(workdir)` fall through to the
/// absolute path — whose components then formed a nonsense descent list and
/// every lookup quietly returned "no committed history found".
fn base_components(prj: &Project, repo: &gix::Repository) -> Vec<String> {
    fn canon(p: &Path) -> PathBuf {
        std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
    }
    // The base is `root.join(config.base)` by construction, and `resolve_base`
    // refuses a base outside the root, so this strip always succeeds.
    let base_rel = prj
        .base
        .strip_prefix(&prj.root)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let root_rel = match repo.workdir() {
        Some(wd) => canon(&prj.root)
            .strip_prefix(canon(wd))
            .map(Path::to_path_buf)
            .unwrap_or_default(),
        None => PathBuf::new(),
    };
    root_rel
        .join(base_rel)
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str().map(str::to_string),
            _ => None,
        })
        .collect()
}

/// Descend `base` within `tree`, returning the inventory base's own tree id —
/// `None` when the inventory did not exist at that commit.
fn base_tree(
    repo: &gix::Repository,
    tree: &gix::Tree,
    base: &[String],
) -> anyhow::Result<Option<ObjectId>> {
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
    Ok(Some(scope.id().detach()))
}

/// Every document blob under an inventory base tree, keyed by **basename** —
/// which is the document's id, and is stable across the relocations a status
/// change causes. Keying by path instead would report `FEAT-0001.md` moving into
/// `_archived/` as a delete plus an add.
fn doc_blobs(
    repo: &gix::Repository,
    tree: ObjectId,
) -> anyhow::Result<BTreeMap<String, (PathBuf, ObjectId)>> {
    fn walk(
        repo: &gix::Repository,
        tree: &gix::Tree,
        prefix: &Path,
        out: &mut BTreeMap<String, (PathBuf, ObjectId)>,
    ) -> anyhow::Result<()> {
        for entry in tree.iter() {
            let entry = entry?;
            let Ok(name) = entry.filename().to_str() else {
                continue;
            };
            if entry.mode().is_tree() {
                let sub = repo.find_tree(entry.oid().to_owned())?;
                walk(repo, &sub, &prefix.join(name), out)?;
            } else if doc_id_of(name).is_some() {
                out.insert(
                    name.to_string(),
                    (prefix.join(name), entry.oid().to_owned()),
                );
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(repo, &repo.find_tree(tree)?, Path::new(""), &mut out)?;
    Ok(out)
}

/// The diff itself, with the decode step supplied by the caller: `decode` is
/// handed the path (relative to the base) and the raw blob of every document
/// that is new or whose content differs, and *only* of those. An unchanged
/// document's blob is never read — which is the property the reader is built
/// for, and the reason this seam is a parameter: a test counts the calls.
pub(crate) fn changed_docs_with<F>(
    repo: &gix::Repository,
    prj: &Project,
    r: &Ref,
    mut decode: F,
) -> anyhow::Result<Changed>
where
    F: FnMut(&Path, &[u8]) -> Option<Doc>,
{
    let base = base_components(prj, repo);
    let main_tree = match truth_branch(repo, &prj.pcfg.git) {
        Some((_, oid)) => base_tree(repo, &repo.find_commit(oid)?.tree()?, &base)?,
        None => None,
    };
    let ref_oid = ObjectId::from_hex(r.commit.as_bytes())?;
    let ref_tree = base_tree(repo, &repo.find_commit(ref_oid)?.tree()?, &base)?;

    // The whole inventory is the same object: nothing to read at all.
    if main_tree == ref_tree {
        return Ok(Changed::default());
    }
    let mine = match ref_tree {
        Some(t) => doc_blobs(repo, t)?,
        None => BTreeMap::new(),
    };
    let theirs = match main_tree {
        Some(t) => doc_blobs(repo, t)?,
        None => BTreeMap::new(),
    };

    let mut docs = Vec::new();
    for (name, (path, oid)) in &mine {
        if theirs.get(name).map(|(_, o)| o) == Some(oid) {
            continue;
        }
        let blob = repo.find_object(*oid)?.into_blob();
        if let Some(doc) = decode(path, &blob.data) {
            docs.push(doc);
        }
    }
    let deleted = theirs
        .keys()
        .filter(|name| !mine.contains_key(*name))
        .filter_map(|name| doc_id_of(name).map(str::to_string))
        .collect();
    Ok(Changed { docs, deleted })
}

/// What one ref holds that main does not: the documents added or modified under
/// the inventory base, decoded through the real [`Doc`] parser, plus the ids it
/// deleted. A blob that fails to parse is skipped — a historical revision may
/// predate the current frontmatter shape, and one such document must not blind
/// the caller to the rest of the branch.
pub fn docs_changed_vs_main(
    repo: &gix::Repository,
    prj: &Project,
    r: &Ref,
) -> anyhow::Result<Changed> {
    changed_docs_with(repo, prj, r, |path, bytes| {
        Doc::parse(prj.base.join(path), &String::from_utf8_lossy(bytes)).ok()
    })
}

/// Which refs hold which documents at a status main's working copy does not.
///
/// `on_main` is main's loaded corpus, supplied by the caller: document I/O is
/// the backend's job, and the engine's git reader deliberately owns no
/// filesystem reads of the corpus. It is also what the callers already have in
/// hand — re-reading it here would be a second, possibly disagreeing, opinion
/// about the same files.
///
/// Nothing fails: a project that is not a repository, an unreadable ref, a
/// broken diff — each yields empty (or partial) occupancy plus a notice.
pub fn occupancy(prj: &Project, on_main: &[Doc]) -> Occupancy {
    let mut occ = Occupancy::default();
    let repo = match open(prj) {
        Ok(Some(repo)) => repo,
        Ok(None) => {
            occ.notices.push(NOT_A_REPOSITORY.to_string());
            return occ;
        }
        Err(e) => {
            occ.notices.push(format!("git: {e}"));
            return occ;
        }
    };
    // Without a truth branch there is no baseline: every ref would diff against
    // an empty tree and report the whole inventory as work in flight.
    if truth_branch(&repo, &prj.pcfg.git).is_none() {
        occ.notices.push(NO_TRUTH_BRANCH.to_string());
        return occ;
    }
    let statuses: BTreeMap<&str, &str> = on_main
        .iter()
        .filter_map(|d| Some((d.id()?, d.status()?)))
        .collect();

    let refs = match refs(&repo, &prj.pcfg.git) {
        Ok(refs) => refs,
        Err(e) => {
            occ.notices.push(format!("git: {e}"));
            return occ;
        }
    };
    for r in &refs {
        let changed = match docs_changed_vs_main(&repo, prj, r) {
            Ok(changed) => changed,
            Err(e) => {
                occ.notices.push(format!("git: {}: {e}", r.label));
                continue;
            }
        };
        for doc in &changed.docs {
            let (Some(id), Some(status)) = (doc.id(), doc.status()) else {
                continue;
            };
            if statuses.get(id) == Some(&status) {
                continue;
            }
            occ.by_doc
                .entry(id.to_string())
                .or_default()
                .push(Occupant {
                    label: r.label.clone(),
                    status: status.to_string(),
                    since: crate::history::entries(&doc.frontmatter)
                        .last()
                        .map(|(k, _)| k.clone()),
                });
        }
    }
    occ
}

/// One content-distinct revision of a document.
pub struct Rev {
    pub short: String,
    pub date: String,
    pub author: String,
    pub status: String,
    pub summary: String,
    /// The commit's *committer* time, Unix seconds — the instant `history
    /// --seed` keys an entry by. (`date` is the same instant rendered in the
    /// commit's own offset, which is what `opys history` prints.)
    pub secs: i64,
}

/// Walk first-parent from HEAD and return one [`Rev`] per content-distinct
/// revision of `id`'s document (newest first). A revision is attributed to the
/// commit that *introduced* it — the one whose blob differs from its first
/// parent's — so unrelated later commits don't steal the credit (BUG-0030). The
/// document is located by its stable id-named blob within the inventory base
/// subtree, so relocations are followed without rename heuristics.
pub fn timeline(
    repo: &gix::Repository,
    prj: &Project,
    basename: &str,
    id: &str,
) -> anyhow::Result<Vec<Rev>> {
    let base = base_components(prj, repo);
    let basename = basename.as_bytes().as_bstr();

    let mut revs = Vec::new();
    // An unborn HEAD (a repository with no commit at all) is not an error: there
    // simply is no timeline, which is exactly what an empty result says.
    let Ok(mut commit) = repo.head_commit() else {
        return Ok(revs);
    };
    let mut cur = find_blob(repo, &commit.tree()?, &base, basename)?;

    loop {
        // The first parent (linear history; merges follow the mainline) and the
        // blob as it stood there — that is what "current" becomes next iteration.
        let parent = match commit.parent_ids().next() {
            Some(pid) => Some(repo.find_commit(pid.detach())?),
            None => None,
        };
        let parent_oid = match &parent {
            Some(p) => find_blob(repo, &p.tree()?, &base, basename)?,
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
                let time = commit.time()?;
                revs.push(Rev {
                    short: commit.id().to_hex_with_len(8).to_string(),
                    date: time.format(gix::date::time::format::SHORT)?,
                    author: commit.author()?.name.to_string(),
                    status,
                    summary: commit.message()?.summary().to_string(),
                    secs: time.seconds,
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
/// repo cannot shadow it. An opys id is globally unique, so at most one matches.
fn find_blob(
    repo: &gix::Repository,
    tree: &gix::Tree,
    base: &[String],
    basename: &BStr,
) -> anyhow::Result<Option<ObjectId>> {
    let Some(scope) = base_tree(repo, tree, base)? else {
        return Ok(None);
    };
    search_tree(repo, &repo.find_tree(scope)?, basename)
}

/// Recursively search `tree` for a blob named `basename`.
fn search_tree(
    repo: &gix::Repository,
    tree: &gix::Tree,
    basename: &BStr,
) -> anyhow::Result<Option<ObjectId>> {
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
