//! Turning the allowlist into the node's data model (ADR-0077).
//!
//! Three jobs: a bounded filesystem scan, expansion of allowlist entries into
//! [`Corpus`] values grouped per project, and suggestions for the user to
//! approve. The scan is the only expensive part and it never runs on a request
//! path — see [`scan`].

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use opys_engine::project_config::ProjectConfig;
use serde::Serialize;
use walkdir::WalkDir;

use crate::registry::{Entry, EntryKind, Registry, DEFAULT_DEPTH};

/// Directories never descended into. Build output, vendored dependencies and
/// caches: they hold no projects of the user's, and they are where the entries
/// are (ADR-0077 measures the difference). Hidden directories are skipped too,
/// which covers `.git`, `.direnv`, `.venv` and friends without naming them.
///
/// This also does correctness work: it stops an `opys.toml` vendored inside
/// `node_modules` from ever being suggested as one of the user's projects.
pub const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "build",
    "dist",
    "out",
    "vendor",
    "third_party",
    "__pycache__",
    "venv",
    "Pods",
    "DerivedData",
    "Binaries",
    "Intermediate",
    "Saved",
];

/// One inventory: a directory holding `opys.toml`, plus where its documents
/// live and which project (and worktree) it belongs to.
#[derive(Debug, Clone, Serialize)]
pub struct Corpus {
    /// Stable id, safe as a URL segment. Same recipe as the backend's lock file
    /// naming: sanitized canonical path, tail-truncated, plus a hash.
    pub cid: String,
    /// The directory holding `opys.toml`, canonicalized.
    pub root: PathBuf,
    /// `root` joined with the config's `base`.
    pub base: PathBuf,
    /// Key of the [`ProjectGroup`] this belongs to.
    pub group: String,
    /// The git branch checked out here, when this is a repo on a branch.
    pub branch: Option<String>,
    /// Whether this is the main worktree of its group.
    pub is_primary: bool,
    /// Why this corpus is unusable, if it is — a config that will not parse,
    /// say. Kept so the API can show the problem instead of hiding the project.
    pub error: Option<String>,
}

/// One project: its main worktree plus every sibling worktree that carries the
/// same inventory.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectGroup {
    pub key: String,
    pub name: String,
    pub corpora: Vec<Corpus>,
}

/// A project the scan found that the user has not allowlisted.
#[derive(Debug, Clone, Serialize)]
pub struct Suggestion {
    pub path: PathBuf,
    pub name: String,
    pub already_allowlisted: bool,
}

/// FNV-1a 64-bit, matching the backend's lock-file hash.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// A stable, filesystem-and-URL-safe id for a path. Pure: same path in, same id
/// out, on any run.
pub fn id_for(path: &Path) -> String {
    let s = path.to_string_lossy();
    let mapped: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    // All-ASCII by construction, so the byte-indexed tail is safe.
    let tail = &mapped[mapped.len().saturating_sub(60)..];
    format!("{tail}-{:016x}", fnv1a64(s.as_bytes()))
}

/// Every directory holding an `opys.toml`, at most `depth` levels below `root`.
///
/// **Never call this on a request path or at startup's critical path.** Over a
/// large home directory it is a half-second of walking (ADR-0077 has the
/// table); it belongs in the background job, on demand, or on a slow timer.
pub fn scan(root: &Path, depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let walker = WalkDir::new(root)
        .max_depth(depth)
        .follow_links(false)
        .same_file_system(true)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_skipped(e.file_name().to_string_lossy().as_ref()));
    for entry in walker.flatten() {
        if entry.file_name() == "opys.toml" && entry.file_type().is_file() {
            if let Some(dir) = entry.path().parent() {
                found.push(dir.to_path_buf());
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Whether a directory name is one the scan refuses to descend into.
pub fn is_skipped(name: &str) -> bool {
    name.starts_with('.') || SKIP_DIRS.contains(&name)
}

/// Project *directories* at most `depth` levels below `root`.
///
/// The `+ 1` is the whole point of this wrapper, and it lives here once: a depth
/// bound counts directories — `depth = 1` means "the projects directly under
/// this one" — while [`scan`] walks to the `opys.toml` inside them, which is one
/// level deeper. Without it the walk stops a level short of what
/// [`Entry::covers`] authorizes, and the CLI ends up refusing to allowlist a
/// project ("already served by the prefix entry") that the node never serves.
///
/// [`Entry::covers`]: crate::registry::Entry::covers
fn scan_projects(root: &Path, depth: usize) -> Vec<PathBuf> {
    scan(root, depth.saturating_add(1))
}

/// Expand the allowlist into project groups. Explicit entries contribute
/// themselves; prefix entries contribute whatever the scan finds beneath them.
///
/// Inherits the scan's cost when the registry holds prefix entries, so this is
/// background work too.
pub fn expand(reg: &Registry) -> Vec<ProjectGroup> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for entry in &reg.entries {
        if entry.error.is_some() {
            continue;
        }
        match entry.kind {
            EntryKind::Project => roots.push(entry.path.clone()),
            EntryKind::Prefix => roots.extend(scan_projects(&entry.path, entry.depth)),
        }
    }
    roots.sort();
    roots.dedup();
    group(&roots)
}

/// Projects the scan found under `root` that are not already allowlisted.
/// Purely advisory — nothing here is ever served until the user adds it.
///
/// `depth` counts project directories, exactly as a prefix entry's does, so
/// `scan --depth N` finds what `add --prefix` at that depth would serve.
pub fn suggest(root: &Path, depth: usize, reg: &Registry) -> Vec<Suggestion> {
    scan_projects(root, depth)
        .into_iter()
        .map(|path| Suggestion {
            name: display_name(&path),
            already_allowlisted: reg.covers(&path),
            path,
        })
        .collect()
}

/// The scan root and depth to suggest from, given the registry: `~` at the
/// default depth unless a prefix entry says otherwise.
pub fn suggest_default(reg: &Registry) -> (PathBuf, usize) {
    let home = crate::registry::expand_tilde("~");
    let depth = reg
        .usable(EntryKind::Prefix)
        .map(|e: &Entry| e.depth)
        .max()
        .unwrap_or(DEFAULT_DEPTH);
    (home, depth)
}

/// What a directory is called, for a human: its last segment, or the whole path
/// when it has none. The one definition of a corpus's name — the union view
/// labels a column with it when git has no branch to offer.
pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Build a [`Corpus`] for a project root, reading its config for `base`.
fn corpus(root: &Path, group: String, branch: Option<String>, is_primary: bool) -> Corpus {
    let (base, error) = match ProjectConfig::load(&root.join("opys.toml")) {
        Ok(cfg) => (root.join(cfg.base), None),
        // A config that will not parse leaves the project visible with the
        // reason attached, rather than silently absent.
        Err(e) => (root.to_path_buf(), Some(e.to_string())),
    };
    Corpus {
        cid: id_for(root),
        root: root.to_path_buf(),
        base,
        group,
        branch,
        is_primary,
        error,
    }
}

/// Group project roots by git repository, pulling in sibling worktrees.
///
/// Roots that share a git common directory are one project. Each group is then
/// asked for its full worktree list, and any worktree carrying `opys.toml` at
/// the same relative path joins the group even if it is outside the allowlist —
/// approving a project approves its worktrees. A root that is not in a repo (or
/// where git is unavailable) becomes a group of one.
pub fn group(roots: &[PathBuf]) -> Vec<ProjectGroup> {
    // common dir (or the root itself, for non-repos) -> member roots
    let mut buckets: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for root in roots {
        let key = git_common_dir(root).unwrap_or_else(|| root.clone());
        buckets.entry(key).or_default().push(root.clone());
    }

    let mut groups = Vec::new();
    for (common, members) in buckets {
        let key = id_for(&common);
        // Ask any member for the repo's worktrees; they all share one repo.
        let worktrees = members
            .first()
            .map(|m| worktree_list(m))
            .unwrap_or_default();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut corpora: Vec<Corpus> = Vec::new();

        if worktrees.is_empty() {
            // Not a repo, or no git: every member stands alone.
            for root in &members {
                if seen.insert(root.clone()) {
                    corpora.push(corpus(root, key.clone(), None, corpora.is_empty()));
                }
            }
        } else {
            // Where the inventory sits inside its worktree, so sibling
            // worktrees can be checked at the same relative path.
            let rel = members
                .first()
                .and_then(|m| {
                    git_toplevel(m).and_then(|top| m.strip_prefix(&top).ok().map(PathBuf::from))
                })
                .unwrap_or_default();
            for (i, wt) in worktrees.iter().enumerate() {
                let root = wt.path.join(&rel);
                if !root.join("opys.toml").is_file() {
                    continue;
                }
                let root = std::fs::canonicalize(&root).unwrap_or(root);
                if seen.insert(root.clone()) {
                    corpora.push(corpus(&root, key.clone(), wt.branch.clone(), i == 0));
                }
            }
            // A member the worktree list did not account for (nested project,
            // odd layout) still belongs to its group.
            for root in &members {
                if seen.insert(root.clone()) {
                    corpora.push(corpus(root, key.clone(), None, corpora.is_empty()));
                }
            }
        }

        if corpora.is_empty() {
            continue;
        }
        let name = corpora
            .iter()
            .find(|c| c.is_primary)
            .or_else(|| corpora.first())
            .map(|c| display_name(&c.root))
            .unwrap_or_default();
        groups.push(ProjectGroup { key, name, corpora });
    }
    groups
}

/// One entry of `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// The checked-out branch, or `None` when detached or bare.
    pub branch: Option<String>,
}

/// Parse `git worktree list --porcelain`. The first entry is the main worktree.
///
/// Pure, so the grouping rules are testable without a repo on disk.
pub fn parse_worktree_list(out: &str) -> Vec<Worktree> {
    let mut list: Vec<Worktree> = Vec::new();
    for line in out.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            list.push(Worktree {
                path: PathBuf::from(path),
                branch: None,
            });
        } else if let Some(branch) = line.strip_prefix("branch ") {
            if let Some(last) = list.last_mut() {
                last.branch = Some(branch.trim_start_matches("refs/heads/").to_string());
            }
        }
    }
    list
}

/// Run a git command in `dir`, returning stdout on success. Any failure — git
/// missing, not a repo, a non-zero exit — is `None`: discovery degrades, it
/// never fails because of git.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_common_dir(root: &Path) -> Option<PathBuf> {
    let raw = git(root, &["rev-parse", "--git-common-dir"])?;
    let path = PathBuf::from(&raw);
    // `--git-common-dir` answers relatively (".git") from inside a worktree.
    let abs = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    Some(std::fs::canonicalize(&abs).unwrap_or(abs))
}

fn git_toplevel(root: &Path) -> Option<PathBuf> {
    let raw = git(root, &["rev-parse", "--show-toplevel"])?;
    let path = PathBuf::from(raw);
    Some(std::fs::canonicalize(&path).unwrap_or(path))
}

fn worktree_list(root: &Path) -> Vec<Worktree> {
    let mut list = git(root, &["worktree", "list", "--porcelain"])
        .map(|out| parse_worktree_list(&out))
        .unwrap_or_default();
    // Branches git already reports as checked out somewhere are not candidates
    // for labelling a *different*, detached worktree.
    let claimed: HashSet<String> = list.iter().filter_map(|w| w.branch.clone()).collect();
    for wt in &mut list {
        if wt.branch.is_none() {
            wt.branch = branch_pointing_at_head(&wt.path, &claimed);
        }
    }
    list
}

/// The branch whose tip is the current commit, for a worktree git calls
/// detached.
///
/// A jj-colocated repo keeps git's HEAD detached permanently, so its porcelain
/// output says `detached` even when the work sits squarely on a branch. Without
/// this, every jj project — and the worktree labels the union view is built
/// around — would show no branch at all. A genuinely detached checkout has no
/// branch pointing at HEAD and still reports `None`.
fn branch_pointing_at_head(path: &Path, claimed: &HashSet<String>) -> Option<String> {
    // `for-each-ref` over refs/heads/, not `branch --points-at`: the latter
    // includes a `(HEAD detached at abc1234)` pseudo-entry, which sorts first
    // and is not a branch name.
    let out = git(
        path,
        &[
            "for-each-ref",
            "--points-at",
            "HEAD",
            "--format=%(refname:short)",
            "refs/heads/",
        ],
    )?;
    // Several branches can share a commit; git's own order (alphabetical) is the
    // tiebreak once the ones checked out elsewhere are excluded.
    out.lines()
        .filter(|s| !s.is_empty())
        .find(|s| !claimed.contains(*s))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_at(dir: &Path, base: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("opys.toml"), format!("base = \"{base}\"\n")).unwrap();
    }

    #[test]
    fn scan_finds_nested_projects_and_respects_the_skip_list() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        project_at(&root.join("a"), "inventory");
        project_at(&root.join("deep/b/c"), "inventory");
        project_at(&root.join("node_modules/pkg"), "inventory");
        project_at(&root.join(".cache/thing"), "inventory");
        project_at(&root.join("rust/target/debug/x"), "inventory");

        let found = scan(root, 10);
        assert!(found.contains(&root.join("a")));
        assert!(found.contains(&root.join("deep/b/c")));
        assert!(
            !found
                .iter()
                .any(|p| p.starts_with(root.join("node_modules"))),
            "vendored projects must not be found: {found:?}"
        );
        assert!(!found.iter().any(|p| p.starts_with(root.join(".cache"))));
        assert!(!found
            .iter()
            .any(|p| p.starts_with(root.join("rust/target"))));
    }

    #[test]
    fn scan_respects_the_depth_bound() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        project_at(&root.join("one"), "inventory");
        project_at(&root.join("a/b/c/d/deep"), "inventory");

        // `opys.toml` in `one/` is two levels down, so depth 2 finds it.
        let shallow = scan(root, 2);
        assert_eq!(shallow, vec![root.join("one")]);
        let deep = scan(root, 10);
        assert_eq!(deep.len(), 2, "got {deep:?}");
    }

    #[test]
    fn id_is_stable_url_safe_and_path_specific() {
        let a = id_for(Path::new("/home/dan/Projects/opys"));
        let b = id_for(Path::new("/home/dan/Projects/opys"));
        let c = id_for(Path::new("/home/dan/Projects/other"));
        assert_eq!(a, b, "same path must give the same id");
        assert_ne!(a, c);
        assert!(
            a.chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'),
            "id must be URL-safe: {a}"
        );
    }

    #[test]
    fn parse_worktree_list_reads_paths_branches_and_detached() {
        let out = "\
worktree /home/dan/Projects/opys
HEAD abc123
branch refs/heads/main

worktree /home/dan/Projects/opys-feature
HEAD def456
branch refs/heads/feature/x

worktree /home/dan/Projects/opys-detached
HEAD 999999
detached
";
        let list = parse_worktree_list(out);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].path, PathBuf::from("/home/dan/Projects/opys"));
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        assert_eq!(list[1].branch.as_deref(), Some("feature/x"));
        assert_eq!(list[2].branch, None, "detached worktrees have no branch");
    }

    #[test]
    fn parse_worktree_list_tolerates_empty_output() {
        assert!(parse_worktree_list("").is_empty());
    }

    #[test]
    fn group_of_non_repos_is_one_group_each() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        project_at(&a, "inventory");
        project_at(&b, "inventory");

        let groups = group(&[a.clone(), b.clone()]);
        assert_eq!(groups.len(), 2, "unrelated non-repo projects do not merge");
        for g in &groups {
            assert_eq!(g.corpora.len(), 1);
            assert!(g.corpora[0].is_primary);
            assert!(g.corpora[0].error.is_none());
        }
        assert!(groups.iter().any(|g| g.name == "a"));
    }

    #[test]
    fn a_broken_config_is_carried_as_an_error_not_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("broken");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("opys.toml"), "base = = nope\n").unwrap();

        let groups = group(std::slice::from_ref(&root));
        assert_eq!(groups.len(), 1, "the project must still be visible");
        let c = &groups[0].corpora[0];
        assert_eq!(c.root, root);
        assert!(c.error.is_some(), "the parse failure must be attached");
    }

    #[test]
    fn base_comes_from_the_config() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proj");
        project_at(&root, "inventory");
        let groups = group(std::slice::from_ref(&root));
        assert_eq!(groups[0].corpora[0].base, root.join("inventory"));
    }

    #[test]
    fn expand_covers_project_and_prefix_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(tmp.path()).unwrap();
        let explicit = home.join("explicit");
        let under = home.join("tree/nested/proj");
        project_at(&explicit, "inventory");
        project_at(&under, "inventory");

        let config = home.join("server.toml");
        std::fs::write(
            &config,
            format!(
                "[[project]]\npath = {:?}\n\n[[prefix]]\npath = {:?}\n",
                explicit.display().to_string(),
                home.join("tree").display().to_string()
            ),
        )
        .unwrap();

        let reg = Registry::load_from(&config).unwrap();
        let groups = expand(&reg);
        let roots: Vec<&PathBuf> = groups
            .iter()
            .flat_map(|g| g.corpora.iter().map(|c| &c.root))
            .collect();
        assert!(
            roots.contains(&&explicit),
            "explicit entry served: {roots:?}"
        );
        assert!(roots.contains(&&under), "prefix entry expanded: {roots:?}");
    }

    /// The depth bound has to mean one thing on both sides. When `covers` counts
    /// a level the walk never reaches, the CLI refuses to allowlist a project
    /// ("already served by the prefix entry") that the node in fact never
    /// serves, and `list` says "serving nothing" in the same breath.
    #[test]
    fn covers_and_expand_agree_at_every_prefix_depth() {
        let tmp = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(tmp.path()).unwrap();
        let work = home.join("work");
        let near = work.join("alpha");
        let far = work.join("a/b/beta");
        project_at(&near, "inventory");
        project_at(&far, "inventory");

        let config = home.join("server.toml");
        for depth in 0..=4 {
            std::fs::write(
                &config,
                format!(
                    "[[prefix]]\npath = {:?}\ndepth = {depth}\n",
                    work.display().to_string()
                ),
            )
            .unwrap();
            let reg = Registry::load_from(&config).unwrap();
            let served: Vec<PathBuf> = expand(&reg)
                .iter()
                .flat_map(|g| g.corpora.iter().map(|c| c.root.clone()))
                .collect();
            for project in [&near, &far] {
                assert_eq!(
                    reg.covers(project),
                    served.contains(project),
                    "depth {depth}: covers and expand disagree about {}",
                    project.display()
                );
            }
        }
    }

    #[test]
    fn suggest_marks_what_is_already_allowlisted() {
        let tmp = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(tmp.path()).unwrap();
        let known = home.join("known");
        let unknown = home.join("unknown");
        project_at(&known, "inventory");
        project_at(&unknown, "inventory");

        let config = home.join("server.toml");
        std::fs::write(
            &config,
            format!("[[project]]\npath = {:?}\n", known.display().to_string()),
        )
        .unwrap();
        let reg = Registry::load_from(&config).unwrap();

        let out = suggest(&home, 10, &reg);
        let known_s = out.iter().find(|s| s.path == known).expect("known listed");
        let unknown_s = out
            .iter()
            .find(|s| s.path == unknown)
            .expect("unknown listed");
        assert!(known_s.already_allowlisted);
        assert!(!unknown_s.already_allowlisted);
        assert_eq!(unknown_s.name, "unknown");
    }
}
