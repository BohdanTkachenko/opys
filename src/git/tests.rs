//! The branch reader over fixtures built with gix alone (see
//! [`super::fixture`]) — never a spawned `git`, so the detached-HEAD worktree a
//! jj-colocated checkout leaves behind is reproducible here.

use std::path::Path;

use super::fixture::{commit_key, Fixture};
use std::collections::BTreeMap;

use super::{
    docs_changed_vs_main, occupancy, open, reader, refs, survey, Occupant, NOT_A_REPOSITORY,
    NO_TRUTH_BRANCH,
};
use crate::doc::Doc;
use crate::project::Project;

const CONFIG: &str = r#"base = "opys"
pad = 4

[types.feature]
prefix = "FEAT"
statuses = ["planned", "partial", "done"]
default_status = "planned"
terminal_statuses = []
tags_required = false
"#;

/// A document carrying a lifecycle map with one entry per listed status, keyed
/// by the fixture's commit times — the shape the reader reads `since` out of.
fn doc_text(id: &str, statuses: &[&str]) -> String {
    let mut out = format!("---\nid: {id}\nstatus: {}\n", statuses[statuses.len() - 1]);
    out.push_str("created: \"2026-01-01T00:00:00.000000Z\"\nhistory:\n");
    for (n, status) in statuses.iter().enumerate() {
        out.push_str(&format!("  \"{}\": {status}\n", commit_key(n)));
    }
    out.push_str(&format!("---\n\n# {id}\n\nBody.\n"));
    out
}

fn parse(path: &Path, text: &str) -> Doc {
    Doc::parse(path.to_path_buf(), text).expect("fixture document parses")
}

fn project(dir: &Path) -> Project {
    Project::open(dir.to_str().expect("utf-8 tempdir")).expect("open the fixture project")
}

/// A project on `main` holding two documents, plus a branch that moved
/// `FEAT-0001` to `partial` and left `FEAT-0002` alone. The working copy is left
/// spelling main, the way a checkout of main actually looks.
fn two_docs_and_a_branch(dir: &Path, branch: &str) -> (Fixture, gix::ObjectId) {
    let mut fx = Fixture::init(dir);
    fx.write("opys.toml", CONFIG);
    fx.write("opys/FEAT-0001.md", doc_text("FEAT-0001", &["planned"]));
    fx.write("opys/FEAT-0002.md", doc_text("FEAT-0002", &["planned"]));
    let main = fx.commit("Alice", "seed the inventory");

    fx.write(
        "opys/FEAT-0001.md",
        doc_text("FEAT-0001", &["planned", "partial"]),
    );
    let tip = fx.commit_to(branch, &[main], "Bob", "start FEAT-0001");
    fx.write("opys/FEAT-0001.md", doc_text("FEAT-0001", &["planned"]));
    (fx, tip)
}

fn on_main(prj: &Project) -> Vec<Doc> {
    ["FEAT-0001", "FEAT-0002"]
        .iter()
        .map(|id| {
            let path = prj.base.join(format!("{id}.md"));
            let text = std::fs::read_to_string(&path).expect("read the fixture document");
            parse(&path, &text)
        })
        .collect()
}

#[test]
fn a_branch_with_no_worktree_appears_in_occupancy_at_its_status() {
    let tmp = tempfile::tempdir().unwrap();
    two_docs_and_a_branch(tmp.path(), "refs/heads/feat-x");
    let prj = project(tmp.path());

    let occ = occupancy(&prj, &on_main(&prj));

    assert!(occ.notices.is_empty(), "notices: {:?}", occ.notices);
    // Only the document the branch actually moved, at the branch's status, with
    // the branch's own last lifecycle key as `since`.
    assert_eq!(
        occ.by_doc.keys().cloned().collect::<Vec<_>>(),
        ["FEAT-0001"]
    );
    assert_eq!(
        occ.by_doc["FEAT-0001"],
        vec![Occupant {
            label: "feat-x".into(),
            status: "partial".into(),
            since: Some(commit_key(1)),
        }]
    );
}

#[test]
fn a_detached_worktree_head_on_a_branch_commit_is_one_column_named_for_the_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let (fx, tip) = two_docs_and_a_branch(tmp.path(), "refs/heads/feat-x");
    // The jj-colocated shape: a linked worktree whose HEAD is a bare object id,
    // parked on the very commit `refs/heads/feat-x` points at.
    fx.worktree_detached("wt", tmp.path().join("checkout-x"), tip);
    let prj = project(tmp.path());
    let repo = open(&prj).unwrap().expect("a repository");

    let found = refs(&repo, &prj.pcfg.git).unwrap();

    assert_eq!(found.len(), 1, "one commit is one column: {found:?}");
    assert_eq!(found[0].label, "feat-x");
    assert_eq!(found[0].name, "refs/heads/feat-x");
    // The label says "feat-x" either way, so the checkout is carried separately:
    // this is how a caller tells a branch somebody is sitting on from one nobody
    // has checked out.
    assert_eq!(found[0].worktrees, [tmp.path().join("checkout-x")]);
    // And it is counted once, not twice.
    let occ = occupancy(&prj, &on_main(&prj));
    assert_eq!(occ.by_doc["FEAT-0001"].len(), 1);
}

#[test]
fn two_worktrees_on_one_commit_and_no_branch_are_labeled_by_directory() {
    let tmp = tempfile::tempdir().unwrap();
    // A ref outside the `[git] branches` patterns: the commit exists and is
    // reachable, but no branch points at it as far as the reader is concerned.
    let (fx, tip) = two_docs_and_a_branch(tmp.path(), "refs/hidden/parked");
    // Admin entry (`<git-dir>/worktrees/<entry>`) and checkout directory are
    // deliberately different strings here: the reader names a worktree by the
    // directory a human sees, not by the entry git filed it under.
    fx.worktree_detached("b", tmp.path().join("wt-b"), tip);
    fx.worktree_detached("a", tmp.path().join("wt-a"), tip);
    let prj = project(tmp.path());
    let repo = open(&prj).unwrap().expect("a repository");

    let found = refs(&repo, &prj.pcfg.git).unwrap();

    // One commit is one column — labelled by both checkout directories, sorted,
    // so neither worktree silently disappears behind the other.
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].label, "wt-a, wt-b");
    // One commit carries one `name`, so it is the first checkout directory in
    // sorted order — never the admin entry, and never all of them.
    assert_eq!(found[0].name, "worktrees/wt-a");
    assert_eq!(found[0].commit, tip.to_hex().to_string());
}

#[test]
fn a_symbolic_worktree_head_resolves_like_a_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let (fx, tip) = two_docs_and_a_branch(tmp.path(), "refs/heads/feat-x");
    // What plain `git worktree add` writes, as opposed to the detached form.
    fx.worktree_on_ref("wt", tmp.path().join("checkout-x"), "refs/heads/feat-x");
    let prj = project(tmp.path());
    let repo = open(&prj).unwrap().expect("a repository");

    let found = refs(&repo, &prj.pcfg.git).unwrap();

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].commit, tip.to_hex().to_string());
    assert_eq!(found[0].label, "feat-x");
}

#[test]
fn a_remote_ref_is_ignored_until_the_branches_patterns_name_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (fx, _) = two_docs_and_a_branch(tmp.path(), "refs/remotes/origin/feat-y");
    let prj = project(tmp.path());
    let repo = open(&prj).unwrap().expect("a repository");

    // Default patterns: local heads only, so a remote-tracking ref is invisible
    // — keeping a remote fetched is the user's job.
    assert!(refs(&repo, &prj.pcfg.git).unwrap().is_empty());
    assert!(occupancy(&prj, &on_main(&prj)).by_doc.is_empty());

    // Named in `[git] branches`, it counts — through the config file, so the
    // table's parsing is part of what this pins.
    fx.write(
        "opys.toml",
        format!("{CONFIG}\n[git]\nbranches = [\"refs/heads/*\", \"refs/remotes/*\"]\n"),
    );
    let prj = project(tmp.path());
    let found = refs(&repo, &prj.pcfg.git).unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].label, "origin/feat-y");
    assert_eq!(
        occupancy(&prj, &on_main(&prj)).by_doc["FEAT-0001"][0].status,
        "partial"
    );
}

#[test]
fn an_unchanged_document_is_never_decoded() {
    let tmp = tempfile::tempdir().unwrap();
    two_docs_and_a_branch(tmp.path(), "refs/heads/feat-x");
    let prj = project(tmp.path());
    let repo = open(&prj).unwrap().expect("a repository");
    let r = refs(&repo, &prj.pcfg.git).unwrap().remove(0);

    // The seam the reader is built around: only blobs whose id differs from
    // main's reach the parser, so an unchanged document costs a tree read and
    // nothing more.
    let mut decoded: Vec<String> = Vec::new();
    let changed = reader::changed_docs_with(&repo, &prj, &r, |path, bytes| {
        decoded.push(path.display().to_string());
        Doc::parse(prj.base.join(path), &String::from_utf8_lossy(bytes)).ok()
    })
    .unwrap();

    assert_eq!(decoded, ["FEAT-0001.md"]);
    assert_eq!(changed.docs.len(), 1);
    assert!(changed.deleted.is_empty());
}

#[test]
fn a_document_deleted_on_a_branch_is_reported_but_does_not_occupy() {
    let tmp = tempfile::tempdir().unwrap();
    let mut fx = Fixture::init(tmp.path());
    fx.write("opys.toml", CONFIG);
    fx.write("opys/FEAT-0001.md", doc_text("FEAT-0001", &["planned"]));
    fx.write("opys/FEAT-0002.md", doc_text("FEAT-0002", &["planned"]));
    let main = fx.commit("Alice", "seed the inventory");
    fx.remove("opys/FEAT-0002.md");
    fx.commit_to("refs/heads/feat-x", &[main], "Bob", "close FEAT-0002");
    fx.write("opys/FEAT-0002.md", doc_text("FEAT-0002", &["planned"]));

    let prj = project(tmp.path());
    let repo = open(&prj).unwrap().expect("a repository");
    let r = refs(&repo, &prj.pcfg.git).unwrap().remove(0);

    let changed = docs_changed_vs_main(&repo, &prj, &r).unwrap();
    assert_eq!(changed.deleted, ["FEAT-0002"]);
    assert!(changed.docs.is_empty());
    // A branch that deleted a document is not occupying it.
    assert!(occupancy(&prj, &on_main(&prj)).by_doc.is_empty());
}

#[test]
fn a_relocated_document_is_followed_by_basename_not_path() {
    let tmp = tempfile::tempdir().unwrap();
    let mut fx = Fixture::init(tmp.path());
    fx.write("opys.toml", CONFIG);
    fx.write("opys/FEAT-0001.md", doc_text("FEAT-0001", &["planned"]));
    let main = fx.commit("Alice", "seed the inventory");
    // What a status change with `status_dirs` does: same document, new path.
    fx.remove("opys/FEAT-0001.md");
    fx.write(
        "opys/_archived/FEAT-0001.md",
        doc_text("FEAT-0001", &["planned", "done"]),
    );
    fx.commit_to("refs/heads/feat-x", &[main], "Bob", "archive it");

    let prj = project(tmp.path());
    let repo = open(&prj).unwrap().expect("a repository");
    let r = refs(&repo, &prj.pcfg.git).unwrap().remove(0);

    let changed = docs_changed_vs_main(&repo, &prj, &r).unwrap();
    // One modified document, not a delete plus an add.
    assert!(changed.deleted.is_empty(), "{:?}", changed.deleted);
    assert_eq!(changed.docs.len(), 1);
    assert_eq!(changed.docs[0].status(), Some("done"));
}

#[test]
fn a_non_repository_yields_empty_occupancy_and_a_single_notice() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("opys.toml"), CONFIG).unwrap();
    std::fs::create_dir_all(tmp.path().join("opys")).unwrap();
    let prj = project(tmp.path());

    let occ = occupancy(&prj, &[]);

    assert!(occ.by_doc.is_empty());
    assert_eq!(occ.notices, [NOT_A_REPOSITORY]);
}

#[test]
fn the_truth_branch_falls_back_to_master_and_is_never_a_column() {
    let tmp = tempfile::tempdir().unwrap();
    let mut fx = Fixture::init(tmp.path());
    fx.write("opys.toml", CONFIG);
    fx.write("opys/FEAT-0001.md", doc_text("FEAT-0001", &["planned"]));
    // A repository that never renamed its default branch: `[git] main` says
    // "main", which resolves to nothing, so `master` is truth.
    let tip = fx.commit_to("refs/heads/master", &[], "Alice", "seed the inventory");
    let prj = project(tmp.path());
    let repo = open(&prj).unwrap().expect("a repository");

    assert_eq!(
        super::truth_branch(&repo, &prj.pcfg.git),
        Some(("refs/heads/master".to_string(), tip))
    );
    // Truth is not progress: it never appears as a column of its own.
    assert!(refs(&repo, &prj.pcfg.git).unwrap().is_empty());
}

#[test]
fn a_repository_with_no_truth_branch_compares_nothing_and_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let mut fx = Fixture::init(tmp.path());
    fx.write("opys.toml", CONFIG);
    fx.write("opys/FEAT-0001.md", doc_text("FEAT-0001", &["planned"]));
    fx.write("opys/FEAT-0002.md", doc_text("FEAT-0002", &["planned"]));
    // Neither `main` nor `master` exists: there is no baseline, so nothing is
    // reported rather than every document on the branch.
    fx.commit_to("refs/heads/feat-x", &[], "Alice", "seed the inventory");
    let prj = project(tmp.path());

    let occ = occupancy(&prj, &on_main(&prj));

    assert!(occ.by_doc.is_empty());
    assert_eq!(occ.notices, [NO_TRUTH_BRANCH]);
}

/// The reader's central promise, pinned by a grep over its own source: nothing
/// under `src/git/` — the gix half, the fixture, these tests — spawns a
/// process. It is the claim `README`, `format.md` and `CLAUDE.md` all make ("in
/// -process through gix, never by spawning `git`"), and the one thing a future
/// "just shell out for this one case" would quietly break: a spawned `git`
/// brings back the machine's `init.defaultBranch`, its identity, and its
/// `safe.directory`, none of which a fixture controls.
///
/// The needle is split so this file does not match itself.
#[test]
fn nothing_in_the_reader_spawns_a_process() {
    let needle = concat!("Command", "::new");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/git");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(
                !text.contains(needle),
                "{} spawns a process; the reader reads git objects with gix",
                path.display()
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 4,
        "expected the whole module, read {checked} files"
    );
}

/// The id → status map [`survey`] takes, from the documents on main.
fn statuses(docs: &[Doc]) -> BTreeMap<String, String> {
    docs.iter()
        .filter_map(|d| Some((d.id()?.to_string(), d.status()?.to_string())))
        .collect()
}

#[test]
fn a_survey_reports_each_ref_with_its_diff_its_occupancy_and_where_to_watch() {
    let tmp = tempfile::tempdir().unwrap();
    two_docs_and_a_branch(tmp.path(), "refs/heads/feat-x");
    let prj = project(tmp.path());

    let surveyed = survey(&prj, &statuses(&on_main(&prj)));

    assert!(surveyed.notices.is_empty(), "{:?}", surveyed.notices);
    assert_eq!(surveyed.refs.len(), 1, "{:?}", surveyed.refs);
    let state = &surveyed.refs[0];
    assert_eq!(state.reference.label, "feat-x");
    assert!(
        state.reference.worktrees.is_empty(),
        "no checkout: {:?}",
        state.reference.worktrees
    );
    // The diff and the occupancy come off one walk, and they say different
    // things: `changed` is what the branch holds, `occupied` the subset whose
    // status main disagrees with.
    let changed = state.changed.as_ref().expect("the diff succeeded");
    assert_eq!(changed.docs.len(), 1);
    assert!(changed.deleted.is_empty());
    assert_eq!(
        state.occupied["FEAT-0001"],
        Occupant {
            label: "feat-x".into(),
            status: "partial".into(),
            since: Some(commit_key(1)),
        }
    );
    // Where to watch for the next change — the common git directory.
    assert_eq!(
        surveyed.git_dir.as_deref().and_then(Path::file_name),
        Some(std::ffi::OsStr::new(".git"))
    );
}

#[test]
fn a_survey_of_a_non_repository_has_nothing_to_watch_and_says_why() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("opys.toml"), CONFIG).unwrap();
    std::fs::create_dir_all(tmp.path().join("opys")).unwrap();
    let prj = project(tmp.path());

    let surveyed = survey(&prj, &BTreeMap::new());

    assert!(surveyed.refs.is_empty());
    assert_eq!(surveyed.notices, [NOT_A_REPOSITORY]);
    // No watcher is worth starting: this answer cannot change without a rescan.
    assert!(surveyed.git_dir.is_none());
}

#[test]
fn a_repository_with_no_truth_branch_is_still_worth_watching() {
    let tmp = tempfile::tempdir().unwrap();
    let mut fx = Fixture::init(tmp.path());
    fx.write("opys.toml", CONFIG);
    fx.write("opys/FEAT-0001.md", doc_text("FEAT-0001", &["planned"]));
    fx.commit_to("refs/heads/feat-x", &[], "Alice", "seed the inventory");
    let prj = project(tmp.path());

    let surveyed = survey(&prj, &BTreeMap::new());

    assert_eq!(surveyed.notices, [NO_TRUTH_BRANCH]);
    assert!(surveyed.refs.is_empty(), "nothing to compare against");
    // `main` may yet be created, and the watcher is what notices.
    assert!(surveyed.git_dir.is_some());
}

/// The three states a survey can be in about its repository, and why only two
/// of them are final: a repository that was found and would not open is a
/// half-written `.git/config` as often as anything else, and a caller that
/// treated it like "there is no repository here" would stop looking forever.
#[test]
fn a_repository_that_will_not_open_is_a_retry_not_a_verdict() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("opys.toml"), CONFIG).unwrap();
    std::fs::create_dir_all(tmp.path().join("opys")).unwrap();
    let git_dir = tmp.path().join(".git");
    std::fs::create_dir_all(git_dir.join("objects")).unwrap();
    std::fs::create_dir_all(git_dir.join("refs")).unwrap();
    std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    // Mid-save: the section header has no closing bracket.
    std::fs::write(git_dir.join("config"), "[core\n\tbare\n").unwrap();

    let surveyed = survey(&project(tmp.path()), &BTreeMap::new());

    assert!(surveyed.git_dir.is_none(), "nothing could be read");
    assert!(surveyed.retry, "{:?}", surveyed.notices);
    assert_eq!(surveyed.notices.len(), 1, "{:?}", surveyed.notices);
    assert!(
        surveyed.notices[0].starts_with("git: "),
        "{:?}",
        surveyed.notices
    );

    // Whereas a directory in no repository at all is settled, and says so.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("opys.toml"), CONFIG).unwrap();
    std::fs::create_dir_all(tmp.path().join("opys")).unwrap();
    let surveyed = survey(&project(tmp.path()), &BTreeMap::new());
    assert_eq!(surveyed.notices, [NOT_A_REPOSITORY]);
    assert!(!surveyed.retry, "there is nothing to try again");
}

/// EACCES on the git directory is discovery's blind spot: its upward walk probes
/// each candidate `.git` and reads *any* failure as "not a repository", so it
/// walks past one that is plainly there and ends with `NoGitRepository`. Taking
/// that at face value reported a broken repository in the exact words a
/// repository-less directory gets — and, for a long-lived caller, ended the
/// thread that would have noticed the permission coming back.
#[cfg(unix)]
#[test]
fn a_git_directory_that_cannot_be_read_is_a_retry_not_a_missing_repository() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    two_docs_and_a_branch(&root, "refs/heads/feat-x");
    let prj = project(&root);
    let git_dir = root.join(".git");
    assert!(
        survey(&prj, &BTreeMap::new()).git_dir.is_some(),
        "it reads before the permission bit is taken away"
    );

    let before = std::fs::metadata(&git_dir).unwrap().permissions();
    std::fs::set_permissions(&git_dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::metadata(git_dir.join("HEAD")).is_ok() {
        // Running as root, where a mode of 000 stops nothing at all.
        std::fs::set_permissions(&git_dir, before).unwrap();
        return;
    }
    let surveyed = survey(&prj, &BTreeMap::new());
    // Restored before the assertions, so a failure still leaves a removable
    // temporary directory behind.
    std::fs::set_permissions(&git_dir, before).unwrap();

    assert!(
        surveyed.retry,
        "the permission can come back: {:?}",
        surveyed.notices
    );
    assert_eq!(surveyed.notices.len(), 1, "{:?}", surveyed.notices);
    assert_ne!(
        surveyed.notices[0], NOT_A_REPOSITORY,
        "a broken repository and an absent one must not read alike"
    );
    assert!(
        surveyed.notices[0].contains("could not be read"),
        "and it says what could not be read: {:?}",
        surveyed.notices
    );

    // And it recovers on the next pass, with nothing restarted.
    let surveyed = survey(&prj, &BTreeMap::new());
    assert!(surveyed.git_dir.is_some(), "{:?}", surveyed.notices);
    assert!(!surveyed.retry);
    assert!(surveyed.notices.is_empty(), "{:?}", surveyed.notices);
}

/// Which directory holds main's copy is a question only the survey can answer,
/// so it hands the caller every checkout — including the main worktree, which
/// gix's own worktree list leaves out.
#[test]
fn a_survey_reports_every_checkout_and_which_of_them_is_on_main() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (fx, tip) = two_docs_and_a_branch(&root, "refs/heads/feat-x");
    let checkout = root.join("checkout-x");
    fx.worktree_detached("wt", &checkout, tip);
    let prj = project(&root);

    let surveyed = survey(&prj, &statuses(&on_main(&prj)));

    let mut got: Vec<(&Path, Option<&str>)> = surveyed
        .checkouts
        .iter()
        .map(|c| (c.root.as_path(), c.branch.as_deref()))
        .collect();
    got.sort();
    assert_eq!(
        got,
        [
            (root.as_path(), Some("main")),
            (checkout.as_path(), Some("feat-x")),
        ],
        "the main worktree, which `gix` does not list, and the detached one — \
         named for the branch it is parked on"
    );

    // And the caller's question, answered off those: `is_on` matches the main
    // worktree by name and the detached checkout by neither.
    let truth = surveyed
        .checkouts
        .iter()
        .find(|c| c.root == root)
        .expect("the main worktree");
    assert_eq!(
        truth.commit.as_deref(),
        Some(
            fx.tip("refs/heads/main")
                .expect("main")
                .to_string()
                .as_str()
        )
    );
}

/// A detached head parked on main's commit is a checkout of main: that is every
/// workspace of a jj-colocated repository, and refusing to recognise it would
/// leave such a project with no baseline at all.
#[test]
fn a_detached_head_on_mains_commit_is_a_checkout_of_main() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (fx, _) = two_docs_and_a_branch(&root, "refs/heads/feat-x");
    let main = fx.tip("refs/heads/main").expect("main exists");
    fx.detach_head(main);
    let prj = project(&root);

    let mut seen: Option<(super::Truth, Vec<super::Checkout>)> = None;
    let surveyed = super::survey_with(&prj, |truth, checkouts| {
        seen = Some((truth.clone(), checkouts.to_vec()));
        Some(statuses(&on_main(&prj)))
    });
    let (truth, checkouts) = seen.expect("the callback was handed the baseline");

    assert_eq!(truth.name, "refs/heads/main");
    assert_eq!(truth.short, "main");
    assert_eq!(truth.commit, main.to_string());
    let here = checkouts
        .iter()
        .find(|c| c.root == root)
        .expect("the main worktree");
    assert_eq!(
        here.branch.as_deref(),
        Some("main"),
        "jj detaches every head, and the branch it is parked on is still the answer"
    );
    assert!(here.is_on(&truth), "and it is still sitting on main");
    assert_eq!(surveyed.refs.len(), 1, "the branch is still read");
}

/// A caller with no baseline stops the survey rather than comparing against
/// nothing — which would report every branch as holding the whole inventory.
#[test]
fn a_survey_with_no_baseline_reads_no_refs_and_still_says_where_to_watch() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    two_docs_and_a_branch(&root, "refs/heads/feat-x");
    let prj = project(&root);

    let surveyed = super::survey_with(&prj, |_, _| None);

    assert!(surveyed.refs.is_empty(), "nothing was compared");
    assert!(
        surveyed.notices.is_empty(),
        "the reason is the caller's to give: {:?}",
        surveyed.notices
    );
    assert!(surveyed.git_dir.is_some(), "still worth watching");
    assert!(!surveyed.checkouts.is_empty(), "and still worth reporting");
}
