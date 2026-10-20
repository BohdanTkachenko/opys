//! The gix-backed half of [`crate::git`] — every item that mentions a git
//! object. Compiled only with the `git` feature; the module's data types and
//! the main-only fallback live in its parent and are always compiled.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use gix::bstr::{BStr, ByteSlice};
use gix::ObjectId;

use super::{
    Changed, Checkout, Occupancy, Occupant, Ref, RefState, Survey, Truth, NOT_A_REPOSITORY,
    NO_TRUTH_BRANCH,
};
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
/// repository is found. A discovery failure is `None` — "no repository here" —
/// and ADR-0053 says that degrades rather than fails; a repository that is found
/// but cannot be opened is a real error.
///
/// The two are told apart by [`unreadable_git_dir`] rather than by the variant
/// discovery returned, because discovery cannot tell them apart either: its
/// upward walk probes each candidate `.git` and treats *any* failure to read one
/// — a permissions bit, an IO error — as "not a repository", so it walks past a
/// repository that is plainly there and ends with `NoGitRepository`. Reporting
/// that as "no repository" is the difference between a state that can clear on
/// its own and a verdict that never changes: a long-lived caller stops watching
/// on the second one (see [`super::Survey::retry`]).
pub fn open(prj: &Project) -> anyhow::Result<Option<gix::Repository>> {
    match gix::discover(&prj.root) {
        Ok(repo) => Ok(Some(repo)),
        Err(gix::discover::Error::Discover(_)) => match unreadable_git_dir(&prj.root) {
            Some(err) => Err(err),
            None => Ok(None),
        },
        Err(e) => Err(e.into()),
    }
}

/// The `.git` that is there and cannot be read, when discovery's "no repository
/// here" is really "I could not look".
///
/// Walks the same ancestors discovery walks and probes each `.git` the way
/// discovery does: the entry itself, and — for a directory — its `HEAD`. Only an
/// IO error *other than* "not found" is reported, so the two ordinary answers
/// are left exactly as they were: no `.git` at all, and a `.git` that reads fine
/// but is not a repository (a stray directory, an unrecognised layout) both stay
/// `None`. What is left is the state the caller has no other way to see — a
/// permissions bit, a half-mounted filesystem, an IO error — and the caller
/// treats it as worth trying again rather than as a verdict.
fn unreadable_git_dir(root: &Path) -> Option<anyhow::Error> {
    let start = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    for dir in start.ancestors() {
        let dot_git = dir.join(".git");
        let probe = match std::fs::symlink_metadata(&dot_git) {
            Ok(md) if md.is_dir() => std::fs::metadata(dot_git.join("HEAD")).map(|_| ()),
            // A `.git` *file* points elsewhere; reading it is the whole probe.
            Ok(_) => std::fs::read(&dot_git).map(|_| ()),
            Err(e) => Err(e),
        };
        match probe {
            Ok(()) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Some(anyhow::anyhow!(
                    "{} could not be read: {e}",
                    dot_git.display()
                ))
            }
        }
    }
    None
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
    /// Checkout directory *names*, for the label.
    worktrees: Vec<String>,
    /// The same checkouts as paths, for a caller asking whether this commit is
    /// checked out anywhere. A worktree whose `gitdir` file cannot be read
    /// contributes a name (its admin entry) but no path.
    paths: Vec<PathBuf>,
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
    read_refs(repo, cfg, false)
}

// Work matching needs every admitted identity; display callers keep their
// traditional one-label-per-commit projection.
fn read_refs(repo: &gix::Repository, cfg: &Git, identities: bool) -> anyhow::Result<Vec<Ref>> {
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
        let base = wt.base().ok();
        let base = base.as_deref().map(Path::to_path_buf);
        let entry = found.entry(id).or_default();
        entry.worktrees.push(worktree_label(&wt, base.as_deref()));
        if let Some(base) = base {
            entry.paths.push(base);
        }
    }

    let mut out: Vec<Ref> = found
        .into_iter()
        .filter(|(oid, _)| Some(*oid) != truth_commit)
        .flat_map(|(oid, mut c)| {
            c.refs.sort();
            c.worktrees.sort();
            c.paths.sort();
            let label = match c.refs.first() {
                Some(name) => short_name(name),
                None => c.worktrees.join(", "),
            };
            let name = match (c.refs.first(), c.worktrees.first()) {
                (Some(name), _) => name.clone(),
                (None, Some(dir)) => format!("worktrees/{dir}"),
                (None, None) => String::new(),
            };
            if identities && !c.refs.is_empty() {
                c.refs
                    .iter()
                    .map(|name| Ref {
                        name: name.clone(),
                        label: short_name(name),
                        commit: oid.to_hex().to_string(),
                        worktrees: c.paths.clone(),
                    })
                    .collect::<Vec<_>>()
            } else {
                vec![Ref {
                    name,
                    commit: oid.to_hex().to_string(),
                    label,
                    worktrees: c.paths,
                }]
            }
        })
        .collect();
    out.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.name.cmp(&b.name)));
    Ok(out)
}

/// The branch and commit a checkout's `HEAD` file names. Both spellings are
/// handled on purpose: plain git writes `ref: refs/heads/x`, a jj-colocated
/// worktree writes a bare object id — and that detached case is the one the
/// union view exists for, so reading only the symbolic form would drop exactly
/// the worktrees that matter.
///
/// Read as a *file* rather than through `repo.head()` because the repository may
/// have been discovered from any checkout: `head()` answers for that one, while
/// every caller here wants a named checkout's own head.
fn head_of(repo: &gix::Repository, git_dir: &Path) -> (Option<String>, Option<ObjectId>) {
    let Ok(text) = std::fs::read_to_string(git_dir.join("HEAD")) else {
        return (None, None);
    };
    let text = text.trim();
    match text.strip_prefix("ref:") {
        Some(name) => {
            let name = name.trim();
            let commit = repo
                .try_find_reference(name)
                .ok()
                .flatten()
                .and_then(|mut r| r.peel_to_id().ok())
                .map(|id| id.detach());
            (Some(short_name(name)), commit)
        }
        None => (None, ObjectId::from_hex(text.as_bytes()).ok()),
    }
}

/// The commit a linked worktree's HEAD names.
fn worktree_head(repo: &gix::Repository, git_dir: &Path) -> Option<ObjectId> {
    head_of(repo, git_dir).1
}

/// Every checkout of this repository: the main worktree plus each linked one,
/// with the branch and commit its `HEAD` names.
///
/// The main worktree is included and `gix::Repository::worktrees` does not list
/// it, so it is derived from the common git directory — which *is*
/// `<main worktree>/.git` in the ordinary layout, and is the only place the main
/// checkout can be found from a linked one. A bare repository has no main
/// worktree, and contributes none.
///
/// A detached head is then given the branch it is parked on, by
/// [`name_detached_heads`] — see there for why that is a branch and not a
/// missing answer.
fn checkouts(repo: &gix::Repository) -> Vec<Checkout> {
    let mut out: Vec<Checkout> = Vec::new();
    let common = repo.common_dir();
    let main_root = if repo.git_dir() == common {
        repo.workdir().map(Path::to_path_buf)
    } else {
        // Opened from a linked worktree: the main checkout is the parent of the
        // shared `.git` directory, when that is what the common directory is.
        (common.file_name() == Some(std::ffi::OsStr::new(".git")))
            .then(|| common.parent().map(Path::to_path_buf))
            .flatten()
    };
    if let Some(root) = main_root {
        let (branch, commit) = head_of(repo, common);
        out.push(Checkout {
            root,
            branch,
            commit: commit.map(|id| id.to_hex().to_string()),
        });
    }
    for wt in repo.worktrees().into_iter().flatten() {
        let Some(root) = wt.base().ok().map(|b| b.to_path_buf()) else {
            continue;
        };
        let (branch, commit) = head_of(repo, wt.git_dir());
        out.push(Checkout {
            root,
            branch,
            commit: commit.map(|id| id.to_hex().to_string()),
        });
    }
    name_detached_heads(repo, &mut out);
    out
}

/// Give every detached checkout the branch whose tip it is parked on.
///
/// A jj-colocated repository detaches *every* head, so without this the answer
/// to "what is this checkout on" would be `None` for every worktree of the
/// repositories this reader mostly runs against — and a caller showing it would
/// have nothing to show. It is the same rule [`Ref::label`] uses one function
/// up: a branch name wins over no name, because the commit is the branch's tip
/// and the branch is what a human calls that work.
///
/// A branch another checkout holds *symbolically* is not offered: git refuses to
/// check one branch out twice, so the detached checkout is not on it, and
/// claiming so would put one name above two columns. Several branches at one
/// commit are broken by the same rule as everywhere here — sorted, first wins —
/// so the answer does not depend on ref iteration order.
fn name_detached_heads(repo: &gix::Repository, checkouts: &mut [Checkout]) {
    if checkouts.iter().all(|c| c.branch.is_some()) {
        return;
    }
    let claimed: BTreeSet<&str> = checkouts
        .iter()
        .filter_map(|c| c.branch.as_deref())
        .collect();
    let mut at: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(platform) = repo.references() {
        if let Ok(heads) = platform.prefixed("refs/heads/") {
            for r in heads.flatten() {
                let mut r = r;
                let name = short_name(&r.name().as_bstr().to_string());
                if claimed.contains(name.as_str()) {
                    continue;
                }
                let Ok(id) = r.peel_to_id() else { continue };
                let commit = id.detach().to_hex().to_string();
                // Sorted, first wins: `prefixed` yields loose and packed refs in
                // no guaranteed joint order, so pick rather than take.
                let slot = at.entry(commit).or_insert_with(|| name.clone());
                if name < *slot {
                    *slot = name;
                }
            }
        }
    }
    for c in checkouts.iter_mut().filter(|c| c.branch.is_none()) {
        c.branch = c.commit.as_ref().and_then(|id| at.get(id)).cloned();
    }
}

/// A worktree's checkout directory name — what a human calls that worktree —
/// falling back to its entry under `worktrees/` when the `gitdir` file cannot be
/// read.
fn worktree_label(wt: &gix::worktree::Proxy<'_>, base: Option<&Path>) -> String {
    base.and_then(Path::file_name)
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

/// One pass of the branch reader: every ref, its diff against main, the
/// documents it occupies, and the git directory to watch.
///
/// `on_main` is main's corpus reduced to id → status, supplied by the caller:
/// document I/O is the backend's job, and the engine's git reader deliberately
/// owns no filesystem reads of the corpus. It is also what the callers already
/// have in hand — re-reading it here would be a second, possibly disagreeing,
/// opinion about the same files.
///
/// Nothing fails. A project that is not a repository, one with no truth branch,
/// a ref listing that broke: each yields no refs plus exactly one notice. A
/// single ref whose diff broke is *not* a notice — it stays in the result with
/// its reason, because a caller drawing columns must draw that column and say it
/// could not answer. See [`Survey`].
pub fn survey(prj: &Project, on_main: &BTreeMap<String, String>) -> Survey {
    survey_with(prj, |_, _| Some(on_main.clone()))
}

#[cfg(test)]
thread_local! { static ANCESTRY_WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

#[cfg(test)]
pub(crate) fn ancestry_walks() -> usize {
    ANCESTRY_WALKS.with(|n| n.get())
}

// One entry per thread bounds memory and avoids a global lock. Commit ancestry
// is immutable; a changed truth tip, repository or shallow boundary invalidates
// it. Failed walks are never cached, and there is no history-depth cutoff.
struct AncestryCache {
    directory: PathBuf,
    main: ObjectId,
    shallow: Option<Vec<u8>>,
    commits: std::sync::Arc<BTreeSet<String>>,
}
thread_local! {
    static WORK_ANCESTRY: std::cell::RefCell<Option<AncestryCache>> = const { std::cell::RefCell::new(None) };
}

fn reachable_from_main(
    repo: &gix::Repository,
    main: ObjectId,
) -> anyhow::Result<std::sync::Arc<BTreeSet<String>>> {
    let directory = repo.common_dir().to_path_buf();
    let shallow = match std::fs::read(directory.join("shallow")) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    WORK_ANCESTRY.with(|slot| {
        if let Some(cached) = slot
            .borrow()
            .as_ref()
            .filter(|c| c.directory == directory && c.main == main && c.shallow == shallow)
        {
            return Ok(cached.commits.clone());
        }
        let mut commits = BTreeSet::new();
        #[cfg(test)]
        ANCESTRY_WALKS.with(|n| n.set(n.get() + 1));
        for info in repo.rev_walk([main]).all()? {
            commits.insert(info?.id.to_hex().to_string());
        }
        let commits = std::sync::Arc::new(commits);
        *slot.borrow_mut() = Some(AncestryCache {
            directory,
            main,
            shallow,
            commits: commits.clone(),
        });
        Ok(commits)
    })
}

/// The actual truth corpus when the invoking checkout is not main. `None`
/// means its loaded corpus already is main, or there is no repository at all.
/// Never substitute a work checkout when a repository's baseline is unreadable.
pub fn work_main(prj: &Project) -> anyhow::Result<Option<Vec<Doc>>> {
    let Some(repo) = open(prj)? else {
        return Ok(None);
    };
    let Some((name, main)) = truth_branch(&repo, &prj.pcfg.git) else {
        anyhow::bail!("no truth branch (`[git] main`, or `master`)");
    };
    let (head, commit) = head_of(&repo, repo.git_dir());
    if head.as_deref() == Some(short_name(&name).as_str())
        || (head.is_none() && commit == Some(main))
    {
        return Ok(None);
    }
    let base = base_components(prj, &repo);
    let Some(tree) = base_tree(&repo, &repo.find_commit(main)?.tree()?, &base)? else {
        return Ok(Some(Vec::new()));
    };
    let mut docs = Vec::new();
    for (_, (path, oid)) in doc_blobs(&repo, tree)? {
        let blob = repo.find_object(oid)?.into_blob();
        docs.push(
            Doc::parse(prj.base.join(path), &String::from_utf8_lossy(&blob.data))
                .map_err(|e| anyhow::anyhow!("reading the truth ref: {e}"))?,
        );
    }
    Ok(Some(docs))
}

/// Work branches stop being open when their tip is reachable from main, even
/// if the orchestrator keeps the ref after merging. Reuse the ordinary survey
/// and walk commit objects in process; never infer a merge from equal statuses.
pub fn work_survey(prj: &Project, on_main: &BTreeMap<String, String>) -> Survey {
    let mut out = survey_with_identities(prj, |_, _| Some(on_main.clone()), true);
    out.refs.retain(|s| {
        on_main.keys().any(|id| {
            prj.pcfg.type_name_for_id(id).is_some_and(|name| {
                prj.pcfg.types[name]
                    .work_branches
                    .values()
                    .any(|w| crate::work::matches_ref(&s.reference, &w.reference, id))
            })
        })
    });
    if out.refs.is_empty() {
        return out;
    }
    let merged = (|| -> anyhow::Result<std::sync::Arc<BTreeSet<String>>> {
        let Some(repo) = open(prj)? else {
            return Ok(Default::default());
        };
        let Some((_, main)) = truth_branch(&repo, &prj.pcfg.git) else {
            return Ok(Default::default());
        };
        reachable_from_main(&repo, main)
    })();
    match merged {
        Ok(commits) => out.refs.retain(|s| !commits.contains(&s.reference.commit)),
        Err(e) => out
            .notices
            .push(format!("git: checking merged work branches: {e}")),
    }
    // The normal reader skips equal blobs. A declared branch still needs its
    // exact copy for shape checks: the caller's main may have uncommitted edits.
    // Read only a matching declaration's missing blob, through the same parser.
    if let Ok(Some(repo)) = open(prj) {
        let base = base_components(prj, &repo);
        for state in &mut out.refs {
            let Ok(changed) = &mut state.changed else {
                continue;
            };
            for id in on_main.keys() {
                let Some(t) = prj.pcfg.type_name_for_id(id).map(|n| &prj.pcfg.types[n]) else {
                    continue;
                };
                if !t
                    .work_branches
                    .values()
                    .any(|w| crate::work::matches_ref(&state.reference, &w.reference, id))
                    || changed.docs.iter().any(|d| d.id() == Some(id))
                    || changed.deleted.contains(id)
                {
                    continue;
                }
                let read = (|| -> anyhow::Result<Option<Doc>> {
                    let oid = ObjectId::from_hex(state.reference.commit.as_bytes())?;
                    let basename = format!("{id}.md");
                    let Some(blob) = find_blob(
                        &repo,
                        &repo.find_commit(oid)?.tree()?,
                        &base,
                        basename.as_bytes().as_bstr(),
                    )?
                    else {
                        return Ok(None);
                    };
                    let blob = repo.find_object(blob)?.into_blob();
                    Ok(Doc::parse(
                        prj.base.join(basename),
                        &String::from_utf8_lossy(&blob.data),
                    )
                    .ok())
                })();
                match read {
                    Ok(Some(doc)) => changed.docs.push(doc),
                    Ok(None) => {}
                    Err(e) => out
                        .notices
                        .push(format!("git: {}: {id}: {e}", state.reference.label)),
                }
            }
        }
    }
    out
}

/// [`survey`] for a caller that cannot say what main *is* until it has been
/// told which branch is truth and where the repository is checked out.
///
/// `on_main` is handed the [`Truth`] branch and every [`Checkout`], and returns
/// the id → status map to measure the refs against — main's inventory. The
/// order is not a nicety: occupancy means "a status main's copy does not have",
/// and a caller serving several directories cannot know *which* of them holds
/// main's copy until it has been told which branch is truth and what each
/// checkout has. One that guessed — the leftmost worktree, say — would measure
/// every branch against some third branch's working copy and call the result
/// main.
///
/// Returning `None` means "I have no baseline": the survey stops there, with its
/// [`Survey::git_dir`] and [`Survey::checkouts`] filled in and no refs, and the
/// caller says why in its own words — the reasons it can have are its own, not
/// git's. Nothing here fails either way.
pub fn survey_with<F>(prj: &Project, on_main: F) -> Survey
where
    F: FnOnce(&Truth, &[Checkout]) -> Option<BTreeMap<String, String>>,
{
    survey_with_identities(prj, on_main, false)
}

fn survey_with_identities<F>(prj: &Project, on_main: F, identities: bool) -> Survey
where
    F: FnOnce(&Truth, &[Checkout]) -> Option<BTreeMap<String, String>>,
{
    let mut out = Survey::default();
    let repo = match open(prj) {
        Ok(Some(repo)) => repo,
        Ok(None) => {
            out.notices.push(NOT_A_REPOSITORY.to_string());
            return out;
        }
        // Discovered and then not openable: a half-written `.git/config`, a
        // permissions blip, an extension gix does not know. Unlike "no
        // repository here", that can be gone by the next pass.
        Err(e) => {
            out.notices.push(format!("git: {e}"));
            out.retry = true;
            return out;
        }
    };
    // Reported even when the read below gives up: a repository with no truth
    // branch is still a repository, and a caller that watches it sees `main`
    // appear rather than waiting for a restart.
    out.git_dir = Some(repo.common_dir().to_path_buf());
    out.checkouts = checkouts(&repo);
    // Without a truth branch there is no baseline: every ref would diff against
    // an empty tree and report the whole inventory as work in flight.
    let Some((name, commit)) = truth_branch(&repo, &prj.pcfg.git) else {
        out.notices.push(NO_TRUTH_BRANCH.to_string());
        return out;
    };
    let truth = Truth {
        short: short_name(&name),
        name,
        commit: commit.to_hex().to_string(),
    };
    let Some(on_main) = on_main(&truth, &out.checkouts) else {
        return out;
    };
    let on_main = &on_main;
    let found = match read_refs(&repo, &prj.pcfg.git, identities) {
        Ok(found) => found,
        Err(e) => {
            out.notices.push(format!("git: {e}"));
            return out;
        }
    };
    let mut by_commit = BTreeMap::new();
    for r in found {
        let changed = by_commit
            .entry(r.commit.clone())
            .or_insert_with(|| docs_changed_vs_main(&repo, prj, &r).map_err(|e| e.to_string()))
            .clone();
        let mut occupied = BTreeMap::new();
        if let Ok(changed) = &changed {
            for doc in &changed.docs {
                let (Some(id), Some(status)) = (doc.id(), doc.status()) else {
                    continue;
                };
                if on_main.get(id).map(String::as_str) == Some(status) {
                    continue;
                }
                occupied.insert(
                    id.to_string(),
                    Occupant {
                        label: r.label.clone(),
                        status: status.to_string(),
                        since: crate::history::entries(&doc.frontmatter)
                            .last()
                            .map(|(k, _)| k.clone()),
                    },
                );
            }
        }
        out.refs.push(RefState {
            reference: r,
            changed,
            occupied,
        });
    }
    out
}

/// Which refs hold which documents at a status main's working copy does not.
///
/// [`survey`] folded down to the shape a one-shot caller wants: the per-ref
/// failures that survey keeps beside their ref become notices here, because a
/// command printing a list has no column to hang them on.
///
/// `on_main` is main's loaded corpus, supplied by the caller: document I/O is
/// the backend's job, and the engine's git reader deliberately owns no
/// filesystem reads of the corpus.
///
/// Nothing fails: a project that is not a repository, an unreadable ref, a
/// broken diff — each yields empty (or partial) occupancy plus a notice.
pub fn occupancy(prj: &Project, on_main: &[Doc]) -> Occupancy {
    let statuses: BTreeMap<String, String> = on_main
        .iter()
        .filter_map(|d| Some((d.id()?.to_string(), d.status()?.to_string())))
        .collect();
    let surveyed = survey(prj, &statuses);
    let mut occ = Occupancy {
        by_doc: BTreeMap::new(),
        notices: surveyed.notices,
    };
    for state in surveyed.refs {
        match state.changed {
            Err(e) => occ
                .notices
                .push(format!("git: {}: {e}", state.reference.label)),
            Ok(_) => {
                for (id, occupant) in state.occupied {
                    occ.by_doc.entry(id).or_default().push(occupant);
                }
            }
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
/// parent's — so unrelated later commits don't steal the credit. The
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
