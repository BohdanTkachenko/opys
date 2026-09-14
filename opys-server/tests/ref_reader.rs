//! The node's ref reader over repositories built with gix alone (ADR-0102).
//!
//! Whole-file gated on **opys-server's own** `git` feature, because that is what
//! forwards the engine's — and because the engine's `test-fixtures` feature
//! deliberately does not imply `git`, so under `--no-default-features` the
//! fixture builder is not compiled at all and neither is anything here.
//!
//! Nothing below spawns `git`: the fixtures are gix write calls and a handful of
//! ordinary file writes under the git directory, which is exactly what the
//! reader is watching for.
#![cfg(feature = "git")]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use opys_engine::git;
use opys_engine::git::fixture::{commit_key, Fixture};
use opys_server::actor::{DocSummary, Event};
use opys_server::refreader::{Baseline, MainDocs, RefReaderHandle, Served, Spawn, BACKSTOP};
use opys_server::union::{union, Branches};
use tokio::sync::broadcast;

const CONFIG: &str = r#"base = "opys"
pad = 4

[types.feature]
prefix = "FEAT"
statuses = ["planned", "partial", "done"]
default_status = "planned"
terminal_statuses = []
tags_required = false
"#;

/// A document carrying a lifecycle map keyed by the fixture's commit times.
fn doc_text(id: &str, statuses: &[&str]) -> String {
    let mut out = format!("---\nid: {id}\nstatus: {}\n", statuses[statuses.len() - 1]);
    out.push_str("created: \"2026-01-01T00:00:00.000000Z\"\nhistory:\n");
    for (n, status) in statuses.iter().enumerate() {
        out.push_str(&format!("  \"{}\": {status}\n", commit_key(n)));
    }
    out.push_str(&format!("---\n\n# {id}\n\nBody.\n"));
    out
}

/// What the primary corpus's actor would answer for the fixture's main: both
/// documents, `planned`, which is what the working copy spells.
fn main_docs() -> Vec<DocSummary> {
    ["FEAT-0001", "FEAT-0002"]
        .iter()
        .map(|id| DocSummary {
            id: (*id).to_string(),
            type_name: "feature".into(),
            status: "planned".into(),
            title: (*id).to_string(),
            tags: Vec::new(),
            path: format!("opys/{id}.md"),
            updated: None,
            blocked: false,
            live_blockers: Vec::new(),
            blocks: 0,
            priority: None,
        })
        .collect()
}

/// A project on `main` holding two documents, plus a branch that moved
/// `FEAT-0001` to `partial`. The working copy is left spelling main.
fn two_docs_and_a_branch(dir: &Path, branch: &str) -> Fixture {
    let mut fx = Fixture::init(dir);
    fx.write("opys.toml", CONFIG);
    fx.write("opys/FEAT-0001.md", doc_text("FEAT-0001", &["planned"]));
    fx.write("opys/FEAT-0002.md", doc_text("FEAT-0002", &["planned"]));
    let main = fx.commit("Alice", "seed the inventory");

    fx.write(
        "opys/FEAT-0001.md",
        doc_text("FEAT-0001", &["planned", "partial"]),
    );
    fx.commit_to(branch, &[main], "Bob", "start FEAT-0001");
    fx.write("opys/FEAT-0001.md", doc_text("FEAT-0001", &["planned"]));
    fx
}

/// What the corpus actor on main would answer, as a test can move it.
///
/// The reader asks for main's documents on *every* pass, and half of what it
/// publishes is defined against them: occupancy is the branch statuses main's
/// copy does not have, and a branch column is main's inventory with a diff over
/// it. A fixed vector would leave both untested for the case they exist for —
/// main moving.
#[derive(Clone)]
struct Main(Arc<Mutex<Result<Vec<DocSummary>, String>>>);

impl Main {
    /// Both documents `planned`, which is what the fixture's working copy
    /// spells.
    fn new() -> Main {
        Main(Arc::new(Mutex::new(Ok(main_docs()))))
    }

    /// Move a document on main, as an edit in that worktree would.
    fn set_status(&self, id: &str, status: &str) {
        let mut guard = self.0.lock().expect("the fake main is not poisoned");
        let docs = guard.as_mut().expect("main is readable");
        let doc = docs.iter_mut().find(|d| d.id == id).expect("a fixture doc");
        doc.status = status.to_string();
    }

    /// Make main unreadable, as a corpus mid-reload or with a broken config is.
    fn fail(&self, why: &str) {
        *self.0.lock().expect("the fake main is not poisoned") = Err(why.to_string());
    }

    /// Readable again, back at the fixture's own statuses.
    fn recover(&self) {
        *self.0.lock().expect("the fake main is not poisoned") = Ok(main_docs());
    }

    fn docs(&self) -> MainDocs {
        let shared = Arc::clone(&self.0);
        Box::new(move || {
            shared
                .lock()
                .expect("the fake main is not poisoned")
                .clone()
        })
    }
}

fn spawn(root: &Path, served: Vec<Served>) -> (RefReaderHandle, broadcast::Receiver<Event>) {
    spawn_against(root, served, &Main::new(), BACKSTOP)
}

/// The same, with the corpus the reader may measure against and the backstop
/// spelled out — a test that wants to watch the timer fire cannot wait a minute.
///
/// The baseline is rooted at the project itself, which is the main worktree of
/// every fixture here unless the test moves its `HEAD`.
fn spawn_against(
    root: &Path,
    served: Vec<Served>,
    main: &Main,
    backstop: Duration,
) -> (RefReaderHandle, broadcast::Receiver<Event>) {
    let (tx, rx) = broadcast::channel(64);
    let handle = RefReaderHandle::spawn(Spawn {
        key: "group-key".to_string(),
        root: root.to_path_buf(),
        served,
        baselines: vec![Baseline {
            root: root.to_path_buf(),
            docs: main.docs(),
        }],
        events: tx,
        shape: "shape".to_string(),
        backstop,
    });
    (handle, rx)
}

/// Wait for the next broadcast event, without needing a tokio runtime. Polling
/// with a deadline, never a sleep that *creates* the condition — the same idiom
/// `tests/actor.rs` uses.
fn next_event(rx: &mut broadcast::Receiver<Event>, timeout: Duration) -> Option<Event> {
    let deadline = Instant::now() + timeout;
    loop {
        match rx.try_recv() {
            Ok(e) => return Some(e),
            Err(broadcast::error::TryRecvError::Empty) => {
                if Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return None,
        }
    }
}

/// Poll until `want` holds or the deadline passes, then return the last answer
/// so a failed assertion can report it.
fn wait_for(
    handle: &RefReaderHandle,
    timeout: Duration,
    want: impl Fn(&Branches) -> bool,
) -> std::sync::Arc<Branches> {
    let deadline = Instant::now() + timeout;
    loop {
        let branches = handle.branches();
        if want(&branches) || Instant::now() >= deadline {
            return branches;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn labels(branches: &Branches) -> Vec<&str> {
    branches
        .columns
        .iter()
        .map(|(column, _)| column.label.as_str())
        .collect()
}

#[test]
fn an_occupied_branch_with_no_worktree_becomes_a_union_column() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    two_docs_and_a_branch(&root, "refs/heads/feat-x");

    let (handle, mut rx) = spawn(&root, Vec::new());
    assert!(handle.read_now(), "the reader answers");
    // The startup pass found a branch, so it announced one — once.
    assert!(matches!(
        next_event(&mut rx, Duration::from_secs(5)),
        Some(Event::RefsChanged { .. })
    ));
    let branches = handle.branches();

    assert!(branches.notices.is_empty(), "{:?}", branches.notices);
    assert_eq!(labels(&branches), ["feat-x"]);
    assert!(branches.read_at.is_some());

    // The column is main's inventory with the branch's change applied — *not*
    // just the changed document, or the merge would report FEAT-0002 as
    // something the branch deleted.
    let (_, docs) = &branches.columns[0];
    let docs = docs.as_ref().expect("the branch was read");
    let mut seen: Vec<(&str, &str)> = docs
        .iter()
        .map(|d| (d.id.as_str(), d.status.as_str()))
        .collect();
    seen.sort();
    assert_eq!(seen, [("FEAT-0001", "partial"), ("FEAT-0002", "planned")]);

    // Occupancy is the subset main disagrees with, with the branch's own last
    // lifecycle key.
    assert_eq!(
        branches.occupancy.keys().cloned().collect::<Vec<_>>(),
        ["FEAT-0001"]
    );
    let occupant = &branches.occupancy["FEAT-0001"][0];
    assert_eq!(occupant.label, "feat-x");
    assert_eq!(occupant.status, "partial");
    assert_eq!(occupant.since.as_deref(), Some(commit_key(1).as_str()));

    // And through the merge, which is what a request actually renders.
    let view = union(&[], &branches, &BTreeSet::new());
    assert_eq!(view.columns.len(), 1);
    assert_eq!(view.columns[0].label, "feat-x");
    let row = view
        .rows
        .iter()
        .find(|r| r.id == "FEAT-0001")
        .expect("a row for the moved document");
    assert_eq!(row.occupants.len(), 1);
    assert!(
        view.rows.iter().all(|r| r.only_in.is_empty()),
        "one column cannot be the only one"
    );

    // A re-read that lands on the same refs announces *nothing*. This is the
    // property the backstop rests on: the client treats a frame it cannot
    // attribute to a corpus as affecting every view, so a minute-by-minute
    // unconditional broadcast would refetch every open tab forever.
    assert!(handle.read_now(), "the reader answers again");
    assert!(
        next_event(&mut rx, Duration::from_millis(500)).is_none(),
        "an unchanged re-read says nothing"
    );

    handle.shutdown();
}

#[test]
fn a_detached_worktree_head_on_a_branch_commit_is_one_column_and_none_when_served() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let fx = two_docs_and_a_branch(&root, "refs/heads/feat-x");
    let tip = fx.tip("refs/heads/feat-x").expect("the branch exists");
    // The jj-colocated shape: a linked worktree detached on the branch's commit.
    let checkout = root.join("checkout-x");
    fx.worktree_detached("wt", &checkout, tip);

    // Nobody is serving that checkout (it carries no `opys.toml`), so the commit
    // is one column — one commit is one column, whichever way it is reachable.
    let (handle, _rx) = spawn(&root, Vec::new());
    assert!(handle.read_now());
    assert_eq!(labels(&handle.branches()), ["feat-x"]);
    handle.shutdown();

    // Serve it, and the column goes away: the checkout is already a corpus
    // column with its own watcher, and two columns for one commit would
    // double-count the work in flight. Matched by *path*, since a detached HEAD
    // has no branch name of its own.
    let (handle, _rx) = spawn(
        &root,
        vec![Served {
            branch: None,
            root: checkout,
        }],
    );
    assert!(handle.read_now());
    let branches = handle.branches();
    assert!(branches.columns.is_empty(), "{:?}", labels(&branches));
    assert!(
        branches.occupancy.is_empty(),
        "a checked-out branch is not an occupant either"
    );
    handle.shutdown();

    // The same, matched by *name*: a plain `git worktree add` leaves a symbolic
    // head, and discovery fills the corpus's branch.
    let (handle, _rx) = spawn(
        &root,
        vec![Served {
            branch: Some("feat-x".to_string()),
            root: root.join("somewhere-else"),
        }],
    );
    assert!(handle.read_now());
    assert!(handle.branches().columns.is_empty());
    handle.shutdown();
}

#[test]
fn a_project_that_is_no_repository_yields_no_columns_one_notice_and_no_thread() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    std::fs::write(root.join("opys.toml"), CONFIG).unwrap();
    std::fs::create_dir_all(root.join("opys")).unwrap();

    let (handle, mut rx) = spawn(
        &root,
        vec![Served {
            branch: None,
            root: root.clone(),
        }],
    );
    let branches = wait_for(&handle, Duration::from_secs(5), |b| b.read_at.is_some());

    assert!(branches.columns.is_empty());
    // A notice, not a broken column: the two states must not look alike.
    assert_eq!(branches.notices, [git::NOT_A_REPOSITORY]);
    // And no claim about the served corpus's branch: an absent entry, not
    // `None`, which would overwrite what discovery recorded with "on no
    // branch".
    assert!(branches.branch_of.is_empty(), "{:?}", branches.branch_of);
    assert!(matches!(
        next_event(&mut rx, Duration::from_secs(2)),
        Some(Event::RefsChanged { .. })
    ));

    // Nothing here can change without the group being rebuilt, so the thread
    // stops rather than waking every minute forever.
    let deadline = Instant::now() + Duration::from_secs(5);
    while handle.read_now() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!handle.read_now(), "no repository, no thread");
}

#[test]
fn a_branch_whose_diff_fails_is_a_column_with_a_reason_not_a_missing_column() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let fx = two_docs_and_a_branch(&root, "refs/heads/feat-x");
    // A loose ref pointing at an object that is in the database but is not a
    // commit: the ref lists and peels fine, and the diff against main cannot be
    // taken. Written as a file, because a ref store in that state is what a
    // half-finished fetch or a hand-edited ref leaves behind.
    let not_a_commit = fx
        .repo
        .write_blob(b"not a commit".as_slice())
        .expect("write a blob")
        .detach();
    let broken = fx.repo.common_dir().join("refs/heads/broken");
    std::fs::create_dir_all(broken.parent().unwrap()).unwrap();
    std::fs::write(&broken, format!("{not_a_commit}\n")).unwrap();

    let (handle, _rx) = spawn(&root, Vec::new());
    assert!(handle.read_now());
    let branches = handle.branches();

    let mut got = labels(&branches);
    got.sort();
    assert_eq!(got, ["broken", "feat-x"], "the broken ref keeps its column");
    let (_, docs) = branches
        .columns
        .iter()
        .find(|(c, _)| c.label == "broken")
        .expect("the broken column");
    assert!(docs.is_err(), "with the reason on it, not empty");
    // A per-ref failure is not a whole-reader notice: the other branch is fine.
    assert!(branches.notices.is_empty(), "{:?}", branches.notices);
    assert!(!branches.occupancy.is_empty(), "feat-x still occupies");

    // The merge turns it into a column that answered nothing, so no row claims
    // this branch deleted the inventory.
    let view = union(&[], &branches, &BTreeSet::new());
    let column = view
        .columns
        .iter()
        .find(|c| c.label == "broken")
        .expect("the column survives the merge");
    assert!(column.error.is_some());
    for row in &view.rows {
        assert!(row.only_in.is_empty(), "{}: {:?}", row.id, row.only_in);
    }
    handle.shutdown();
}

#[test]
fn a_ref_change_fires_one_event_and_a_burst_of_them_coalesces() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut fx = two_docs_and_a_branch(&root, "refs/heads/feat-x");
    let tip = fx.tip("refs/heads/feat-x").expect("the branch exists");
    // A second branch, built now and taken back out of the ref store, so it can
    // be re-created later as a change the watcher must notice.
    fx.write(
        "opys/FEAT-0002.md",
        doc_text("FEAT-0002", &["planned", "done"]),
    );
    let second = fx.commit_to("refs/heads/feat-y", &[tip], "Carol", "finish FEAT-0002");
    let feat_y: PathBuf = fx.repo.common_dir().join("refs/heads/feat-y");
    std::fs::remove_file(&feat_y).unwrap();
    fx.write("opys/FEAT-0002.md", doc_text("FEAT-0002", &["planned"]));

    let (handle, mut rx) = spawn(&root, Vec::new());
    // The first pass has something to say, so it says it once.
    assert!(matches!(
        next_event(&mut rx, Duration::from_secs(5)),
        Some(Event::RefsChanged { .. })
    ));
    wait_for(&handle, Duration::from_secs(5), |b| b.columns.len() == 1);

    // A burst: git rewrites a ref as a file, and a fetch or a rebase rewrites
    // several in a row. One quiet period, one read, one event.
    for _ in 0..5 {
        std::fs::write(&feat_y, format!("{second}\n")).unwrap();
    }

    assert!(
        matches!(
            next_event(&mut rx, Duration::from_secs(5)),
            Some(Event::RefsChanged { project }) if project == "group-key"
        ),
        "the watcher noticed the new ref"
    );
    let branches = wait_for(&handle, Duration::from_secs(5), |b| b.columns.len() == 2);
    let mut got = labels(&branches);
    got.sort();
    assert_eq!(got, ["feat-x", "feat-y"]);

    // And nothing further: five writes were one change, and a quiet reader does
    // not announce its periodic re-read.
    assert!(
        next_event(&mut rx, Duration::from_millis(800)).is_none(),
        "the debounce coalesced the burst into one event"
    );
    handle.shutdown();
}

/// A config that will not parse is routinely transient — an editor mid-save —
/// so it must not be the end of the reader. The pass says so and the thread
/// stays, watcher and all, for the read that follows.
#[test]
fn a_config_that_will_not_parse_is_reported_without_killing_the_reader() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let fx = two_docs_and_a_branch(&root, "refs/heads/feat-x");
    // The working copy only: the committed config is still the good one, which
    // is exactly the shape of a half-typed edit.
    fx.write("opys.toml", "this is not toml = = =\n");

    let (handle, _rx) = spawn(&root, Vec::new());
    assert!(handle.read_now());
    let branches = handle.branches();
    assert!(branches.columns.is_empty());
    assert_eq!(branches.notices.len(), 1, "{:?}", branches.notices);
    assert!(
        branches.notices[0].starts_with("branches could not be read: "),
        "{:?}",
        branches.notices
    );

    // Put it back, and the very next pass answers properly — the thread is
    // still there to run it.
    fx.write("opys.toml", CONFIG);
    assert!(handle.read_now(), "the reader did not give up");
    let branches = handle.branches();
    assert!(branches.notices.is_empty(), "{:?}", branches.notices);
    assert_eq!(labels(&branches), ["feat-x"]);
    handle.shutdown();
}

/// The backstop, and the reason it exists. Occupancy is defined against main's
/// **working copy**, which is not under `.git`, so no ref event can fire for it:
/// the only thing that notices main moving is the periodic pass. Both halves are
/// asserted here — that the answer follows main, and that nothing had to ask.
#[test]
fn main_moving_changes_the_answer_and_the_backstop_is_what_notices() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    two_docs_and_a_branch(&root, "refs/heads/feat-x");

    let main = Main::new();
    let (handle, mut rx) = spawn_against(&root, Vec::new(), &main, Duration::from_millis(50));
    assert!(matches!(
        next_event(&mut rx, Duration::from_secs(5)),
        Some(Event::RefsChanged { .. })
    ));
    let branches = wait_for(&handle, Duration::from_secs(5), |b| !b.occupancy.is_empty());
    assert_eq!(
        branches.occupancy.keys().cloned().collect::<Vec<_>>(),
        ["FEAT-0001"],
        "the branch holds it at a status main's copy does not"
    );

    // main catches up with the branch — an edit in that worktree, with no ref
    // written anywhere. Nothing below asks the reader to look.
    main.set_status("FEAT-0001", "partial");

    assert!(
        matches!(
            next_event(&mut rx, Duration::from_secs(5)),
            Some(Event::RefsChanged { project }) if project == "group-key"
        ),
        "an unprompted pass noticed main move"
    );
    let branches = wait_for(&handle, Duration::from_secs(5), |b| b.occupancy.is_empty());
    assert!(
        branches.occupancy.is_empty(),
        "nothing is in flight once main agrees: {:?}",
        branches.occupancy
    );
    // The column is still there — the branch still holds the document, it just
    // no longer holds it at a status main disagrees with.
    assert_eq!(labels(&branches), ["feat-x"]);
    handle.shutdown();
}

/// A baseline that cannot be read is not a baseline. Comparing against nothing
/// would report every branch as holding the whole inventory, so the pass says so
/// and invents no columns.
#[test]
fn a_baseline_that_cannot_be_read_is_a_notice_rather_than_an_invented_column() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    two_docs_and_a_branch(&root, "refs/heads/feat-x");

    let main = Main::new();
    main.fail("corpus is not loaded: bad config");
    let (handle, _rx) = spawn_against(&root, Vec::new(), &main, BACKSTOP);
    assert!(handle.read_now());
    let branches = handle.branches();

    assert!(branches.columns.is_empty(), "{:?}", labels(&branches));
    assert!(branches.occupancy.is_empty());
    assert_eq!(branches.notices.len(), 1, "{:?}", branches.notices);
    assert!(
        branches.notices[0].starts_with("the worktree on `main` could not be read, so branches"),
        "{:?}",
        branches.notices
    );
    assert!(
        branches.notices[0].contains("bad config"),
        "with the corpus's own reason: {:?}",
        branches.notices
    );

    // And when it comes back, so do the branches: the thread stayed.
    main.recover();
    assert!(handle.read_now(), "the reader did not give up");
    let branches = handle.branches();
    assert!(branches.notices.is_empty(), "{:?}", branches.notices);
    assert_eq!(labels(&branches), ["feat-x"]);
    handle.shutdown();
}

/// The worktree the node serves is not necessarily *main's*. When it is not, a
/// branch column would be built from a third branch's working copy and occupancy
/// measured against it — so the reader says it cannot compare instead.
#[test]
fn a_served_worktree_on_another_branch_is_not_main() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut fx = two_docs_and_a_branch(&root, "refs/heads/feat-x");
    // A third branch, and the main worktree checked out on it — an ordinary
    // afternoon in a single-checkout repository.
    let tip = fx.tip("refs/heads/main").expect("main exists");
    fx.write(
        "opys/FEAT-0002.md",
        doc_text("FEAT-0002", &["planned", "done"]),
    );
    fx.commit_to("refs/heads/feat-z", &[tip], "Dana", "finish FEAT-0002");
    fx.write("opys/FEAT-0002.md", doc_text("FEAT-0002", &["planned"]));
    fx.set_head("refs/heads/feat-z");

    // A jj-colocated checkout: git reports no branch for it, so the older
    // name rule has nothing to match and the commit is what decides.
    let (handle, _rx) = spawn(
        &root,
        vec![Served {
            branch: None,
            root: root.clone(),
        }],
    );
    assert!(handle.read_now());
    let branches = handle.branches();

    assert!(
        branches.columns.is_empty(),
        "a column filled in from feat-z is not a column for feat-x: {:?}",
        labels(&branches)
    );
    assert!(
        branches.occupancy.is_empty(),
        "and nothing is measured against feat-z either"
    );
    assert_eq!(
        branches.notices,
        [
            "no served worktree is on `main` — branches are compared against main's copy, \
          so there is nothing here to compare them with"
        ]
    );

    // And the pass that says nothing is on `main` also says what the served
    // corpus *is* on. The two travel together on purpose: a heading still
    // spelling `main` above this notice is the same screen contradicting
    // itself.
    assert_eq!(
        branches.branch_of.get(&root),
        Some(&Some("feat-z".to_string())),
        "{:?}",
        branches.branch_of
    );

    // Check main out again and the comparison is available: same reader, same
    // pass, nothing restarted.
    fx.set_head("refs/heads/main");
    assert!(handle.read_now());
    let branches = handle.branches();
    assert!(branches.notices.is_empty(), "{:?}", branches.notices);
    let mut got = labels(&branches);
    got.sort();
    assert_eq!(got, ["feat-x", "feat-z"]);
    assert_eq!(
        branches.branch_of.get(&root),
        Some(&Some("main".to_string())),
        "and the label follows it back: {:?}",
        branches.branch_of
    );
    handle.shutdown();
}

/// Discovery reads a corpus's branch once, when it expands the allowlist, and
/// the manager rebuilds a group only on a rescan — so a checkout that moves
/// under a running node kept the name it was found on, in the sidebar and in the
/// union's column headings, while the reader's own notice said no served
/// worktree was on main. The reader republishes the branch, and announces it, so
/// the label is as fresh as the notice.
#[test]
fn a_checkout_that_moves_is_republished_with_the_branch_it_moved_to() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let fx = two_docs_and_a_branch(&root, "refs/heads/feat-x");

    // What discovery recorded when it found this corpus: `main`.
    let (handle, mut rx) = spawn(
        &root,
        vec![Served {
            branch: Some("main".to_string()),
            root: root.clone(),
        }],
    );
    assert!(handle.read_now(), "the reader answers");
    assert_eq!(
        handle.branches().branch_of.get(&root),
        Some(&Some("main".to_string()))
    );
    while next_event(&mut rx, Duration::from_millis(200)).is_some() {}

    // The user checks another branch out. Nothing about the *columns* moved —
    // `feat-x` is still the one branch with no checkout — so only the fresh
    // label can be what makes this a change worth announcing.
    fx.set_head("refs/heads/feat-x");
    assert!(handle.read_now());
    let branches = handle.branches();
    assert_eq!(
        branches.branch_of.get(&root),
        Some(&Some("feat-x".to_string())),
        "{:?}",
        branches.branch_of
    );
    assert!(
        matches!(
            next_event(&mut rx, Duration::from_secs(5)),
            Some(Event::RefsChanged { .. })
        ),
        "an open browser is told, or it renders the old name until it reloads"
    );
    handle.shutdown();
}

/// A ref a served corpus has checked out gets no column — including when some
/// *other* branch name sorts first at that commit, which is all a `git::Ref`
/// carries, and when the checkout is the repository's **main** worktree, which
/// appears in no worktree list. Two columns for one commit double-count the work
/// in flight, and the duplicate here would be of the checkout on screen.
#[test]
fn a_second_branch_at_a_served_commit_does_not_duplicate_its_column() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut fx = two_docs_and_a_branch(&root, "refs/heads/feat-x");
    let tip = fx.tip("refs/heads/main").expect("main exists");
    fx.write(
        "opys/FEAT-0002.md",
        doc_text("FEAT-0002", &["planned", "done"]),
    );
    let topic = fx.commit_to("refs/heads/topic", &[tip], "Dana", "finish FEAT-0002");
    fx.write("opys/FEAT-0002.md", doc_text("FEAT-0002", &["planned"]));
    // A leftover branch — or a jj bookmark exported at the same commit — that
    // sorts before the one the main worktree is on, so it wins `Ref::label`.
    // Written as a file: a ref transaction wants a committer, and a ref store in
    // this state is what a half-finished fetch leaves behind anyway.
    std::fs::write(
        fx.repo.common_dir().join("refs/heads/alpha"),
        format!("{topic}\n"),
    )
    .unwrap();
    fx.set_head("refs/heads/topic");
    // Main is checked out elsewhere, so there is still a baseline to measure
    // against — this test is about the *other* checkout.
    let on_main = root.join("wt-main");
    fx.worktree_detached("wt-main", &on_main, tip);

    let (tx, _rx) = broadcast::channel(64);
    let main = Main::new();
    let handle = RefReaderHandle::spawn(Spawn {
        key: "group-key".to_string(),
        root: root.clone(),
        // The corpus in the repository's main worktree: git reports its branch
        // as `topic`, and no worktree list mentions its directory.
        served: vec![Served {
            branch: Some("topic".into()),
            root: root.clone(),
        }],
        baselines: vec![Baseline {
            root: on_main.clone(),
            docs: main.docs(),
        }],
        events: tx,
        shape: "shape".to_string(),
        backstop: BACKSTOP,
    });
    assert!(handle.read_now());
    let branches = handle.branches();

    assert!(branches.notices.is_empty(), "{:?}", branches.notices);
    assert_eq!(
        labels(&branches),
        ["feat-x"],
        "the commit `topic` sits on is already the served column, whatever \
         `alpha` is doing at it"
    );
    assert!(
        !branches.occupancy.contains_key("FEAT-0002"),
        "and a checkout does not occupy itself: {:?}",
        branches.occupancy
    );
    handle.shutdown();
}

/// The `worktrees/*/HEAD` watch, on the repository that had no `worktrees`
/// directory when the reader started — which is most of them, and is the shape
/// ADR-0102 is built around: a jj workspace head is a bare object id that moves
/// without touching `refs/`.
#[test]
fn a_worktree_head_moving_is_a_change_even_when_none_existed_at_startup() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut fx = two_docs_and_a_branch(&root, "refs/heads/feat-x");
    let tip = fx.tip("refs/heads/main").expect("main exists");
    // A commit reachable through nothing but a worktree head, once the ref that
    // built it is taken back out of the store.
    fx.write(
        "opys/FEAT-0002.md",
        doc_text("FEAT-0002", &["planned", "done"]),
    );
    let parked = fx.commit_to("refs/heads/feat-y", &[tip], "Carol", "finish FEAT-0002");
    fx.write("opys/FEAT-0002.md", doc_text("FEAT-0002", &["planned"]));
    let git_dir = fx.repo.common_dir().to_path_buf();
    std::fs::remove_file(git_dir.join("refs/heads/feat-y")).unwrap();
    assert!(
        !git_dir.join("worktrees").exists(),
        "the case: no linked worktree exists yet"
    );

    let (handle, mut rx) = spawn(&root, Vec::new());
    assert!(matches!(
        next_event(&mut rx, Duration::from_secs(5)),
        Some(Event::RefsChanged { .. })
    ));
    wait_for(&handle, Duration::from_secs(5), |b| b.columns.len() == 1);

    // The repository grows its first linked worktree, parked where main is —
    // no new column, so nothing to announce yet. The pass is what re-attempts
    // the watch that could not be attached at startup.
    let checkout = root.join("checkout-x");
    fx.worktree_detached("wt", &checkout, tip);
    assert!(handle.read_now());
    while next_event(&mut rx, Duration::from_millis(200)).is_some() {}

    // Now the workspace head moves, and nothing under `refs/` is written.
    std::fs::write(git_dir.join("worktrees/wt/HEAD"), format!("{parked}\n")).unwrap();

    assert!(
        matches!(
            next_event(&mut rx, Duration::from_secs(5)),
            Some(Event::RefsChanged { project }) if project == "group-key"
        ),
        "the worktree head is watched, even though it appeared after startup"
    );
    let branches = wait_for(&handle, Duration::from_secs(5), |b| b.columns.len() == 2);
    let mut got = labels(&branches);
    got.sort();
    assert_eq!(got, ["checkout-x", "feat-x"]);
    handle.shutdown();
}
