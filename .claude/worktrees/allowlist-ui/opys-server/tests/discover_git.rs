//! Discovery against a real git repository with a second worktree.
//!
//! The pure grouping logic is unit-tested in `discover.rs`; this covers the part
//! that only git can answer — that a project's worktrees collapse into one
//! group, with the right primary and branches, and that approving the main
//! worktree implicitly approves its siblings (ADR-0077).
//!
//! Skips cleanly, with a message, when git is not installed.

use std::path::{Path, PathBuf};
use std::process::Command;

use opys_server::discover;
use opys_server::registry::Registry;

fn have_git() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Run git in `dir`, panicking with git's own stderr when it fails — a broken
/// fixture should say why, not just assert false.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        // Never let the developer's global config (signing, hooks, default
        // branch, identity) decide whether this test passes.
        .args(["-c", "commit.gpgsign=false"])
        .args(["-c", "user.email=test@example.com"])
        .args(["-c", "user.name=Test"])
        .args(["-c", "init.defaultBranch=main"])
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repo with an inventory committed, plus a second worktree on its own branch.
fn repo_with_worktree(tmp: &Path) -> (PathBuf, PathBuf) {
    let main = tmp.join("proj");
    std::fs::create_dir_all(main.join("inventory")).unwrap();
    std::fs::write(main.join("opys.toml"), "base = \"inventory\"\n").unwrap();
    std::fs::write(main.join("inventory/.keep"), "").unwrap();

    git(&main, &["init"]);
    git(&main, &["add", "-A"]);
    git(&main, &["commit", "-m", "init"]);

    let feature = tmp.join("proj-feature");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-b",
            "feature/x",
            feature.to_str().unwrap(),
        ],
    );
    (
        std::fs::canonicalize(&main).unwrap(),
        std::fs::canonicalize(&feature).unwrap(),
    )
}

#[test]
fn worktrees_collapse_into_one_group_with_primary_and_branches() {
    if !have_git() {
        eprintln!("skipping worktrees_collapse_into_one_group_with_primary_and_branches: git is not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let tmp_path = std::fs::canonicalize(tmp.path()).unwrap();
    let (main, feature) = repo_with_worktree(&tmp_path);

    // Only the main worktree is named; the sibling must be pulled in by git.
    let groups = discover::group(std::slice::from_ref(&main));

    assert_eq!(
        groups.len(),
        1,
        "two worktrees are one project: {groups:#?}"
    );
    let g = &groups[0];
    assert_eq!(g.corpora.len(), 2, "both worktrees are corpora: {g:#?}");
    assert_eq!(g.name, "proj");

    let primary = g.corpora.iter().find(|c| c.is_primary).expect("a primary");
    let secondary = g
        .corpora
        .iter()
        .find(|c| !c.is_primary)
        .expect("a secondary");
    assert_eq!(primary.root, main);
    assert_eq!(secondary.root, feature);
    assert_eq!(secondary.branch.as_deref(), Some("feature/x"));
    assert_eq!(primary.branch.as_deref(), Some("main"));

    // Each corpus resolves its own base, and the two are genuinely separate
    // inventories that happen to share a project.
    assert_eq!(primary.base, main.join("inventory"));
    assert_eq!(secondary.base, feature.join("inventory"));
    assert_ne!(
        primary.cid, secondary.cid,
        "ids are per corpus, not per project"
    );
    assert!(g.corpora.iter().all(|c| c.error.is_none()), "{g:#?}");
}

/// jj-colocated repos keep git's HEAD detached, so the porcelain output says
/// `detached` for a checkout that is really sitting on a branch. The label has
/// to survive that, or every jj project shows a blank branch.
#[test]
fn a_detached_head_still_reports_the_branch_at_that_commit() {
    if !have_git() {
        eprintln!(
            "skipping a_detached_head_still_reports_the_branch_at_that_commit: git is not on PATH"
        );
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let tmp_path = std::fs::canonicalize(tmp.path()).unwrap();
    let (main, _feature) = repo_with_worktree(&tmp_path);

    // Exactly what jj leaves behind: HEAD detached at the branch's tip.
    git(&main, &["checkout", "--detach"]);
    assert_eq!(
        git(&main, &["branch", "--show-current"]),
        "",
        "fixture should be detached"
    );

    let groups = discover::group(std::slice::from_ref(&main));
    let primary = groups[0]
        .corpora
        .iter()
        .find(|c| c.root == main)
        .expect("the main worktree");
    assert_eq!(
        primary.branch.as_deref(),
        Some("main"),
        "a detached HEAD on a branch tip should still be labelled"
    );
}

#[test]
fn allowlisting_a_project_serves_its_worktrees() {
    if !have_git() {
        eprintln!("skipping allowlisting_a_project_serves_its_worktrees: git is not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let tmp_path = std::fs::canonicalize(tmp.path()).unwrap();
    let (main, feature) = repo_with_worktree(&tmp_path);

    let config = tmp_path.join("server.toml");
    std::fs::write(
        &config,
        format!("[[project]]\npath = {:?}\n", main.display().to_string()),
    )
    .unwrap();
    let reg = Registry::load_from(&config).unwrap();

    let roots: Vec<PathBuf> = discover::expand(&reg)
        .into_iter()
        .flat_map(|g| g.corpora.into_iter().map(|c| c.root))
        .collect();

    assert!(roots.contains(&main));
    assert!(
        roots.contains(&feature),
        "approving a project approves its worktrees: {roots:?}"
    );
    // The sibling is served, but it is not in the allowlist file itself.
    assert!(
        !reg.covers(&feature),
        "the registry itself still lists only the project"
    );
}
