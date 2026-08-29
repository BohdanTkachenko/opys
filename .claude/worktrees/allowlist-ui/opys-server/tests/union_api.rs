//! `GET /api/group/{key}/union` over a real repository with a second worktree
//! (TASK-0073).
//!
//! The merge itself is unit-tested in `src/union.rs`, where the inputs can be
//! written by hand. What only a real fixture can show is the whole path: git
//! grouping two worktrees into one project, two corpus actors loading two
//! genuinely different inventories, and the route labelling the columns and
//! reporting the drift between them.
//!
//! Skips cleanly, with a message, when git is not installed — a two-corpus group
//! cannot be faked, and every assertion here needs one.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use opys_backend_markdown_local::MarkdownLocal;
use opys_engine::backend::Backend;
use opys_server::api::{self, AppState};
use opys_server::manager::Manager;
use serde_json::Value;
use tokio::sync::broadcast;
use tower::ServiceExt;

const CONFIG: &str = r#"
base = "inventory"

[types.note]
prefix = "NOTE"
statuses = ["open", "closed"]
default_status = "open"
terminal_statuses = ["closed"]
tags_required = false

[types.task]
prefix = "TASK"
statuses = ["todo", "doing", "done"]
default_status = "todo"
terminal_statuses = ["done"]
tags_required = false
"#;

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
        // branch, identity) decide whether this test passes: the column labels
        // asserted below are branch names.
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

fn write_doc(inventory: &Path, id: &str, status: &str, title: &str, extra: &str) {
    std::fs::write(
        inventory.join(format!("{id}.md")),
        format!("---\nid: {id}\nstatus: {status}\n{extra}---\n\n# {title}\n\nBody.\n"),
    )
    .unwrap();
}

fn backend() -> Box<dyn Backend + Send> {
    Box::new(MarkdownLocal)
}

/// A project on `main` with a `feature/x` worktree that has diverged from it.
///
/// Every write lands before the manager starts: `rescan` returns once the actor
/// threads are spawned and reads block on the startup load, so a fixture written
/// in this order needs no sleeping. Writing afterwards would race the watcher's
/// debounce instead.
fn repo_with_diverged_worktree(tmp: &Path) -> (PathBuf, PathBuf) {
    let main = tmp.join("proj");
    let inventory = main.join("inventory");
    std::fs::create_dir_all(&inventory).unwrap();
    std::fs::write(main.join("opys.toml"), CONFIG).unwrap();
    // Committed, so the second worktree starts out identical.
    write_doc(
        &inventory,
        "NOTE-0001",
        "open",
        "Shared note",
        "updated: \"2026-01-01T00:00:00Z\"\n",
    );
    write_doc(&inventory, "TASK-0002", "doing", "Do the thing", "");

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

    // Now the divergence, as working-tree edits: this is what the view exists to
    // present, and it is exactly what a user has in front of them mid-branch.
    let branch_inventory = feature.join("inventory");
    // Same document, further along on the branch.
    write_doc(&branch_inventory, "TASK-0002", "done", "Do the thing", "");
    // Written on the branch, nowhere else.
    write_doc(&branch_inventory, "TASK-0003", "todo", "Branch work", "");
    // Both branches allocated 0004 — an impending id collision.
    write_doc(&inventory, "NOTE-0004", "open", "Numbered four", "");
    write_doc(&branch_inventory, "TASK-0004", "todo", "Also four", "");

    (
        std::fs::canonicalize(&main).unwrap(),
        std::fs::canonicalize(&feature).unwrap(),
    )
}

/// A plain, non-git project: its own group, of one corpus.
fn solo_project(root: &Path) -> PathBuf {
    let inventory = root.join("inventory");
    std::fs::create_dir_all(&inventory).unwrap();
    std::fs::write(root.join("opys.toml"), CONFIG).unwrap();
    write_doc(&inventory, "NOTE-0009", "open", "Alone", "");
    std::fs::canonicalize(root).unwrap()
}

/// A served node over the fixture above.
///
/// Field order is load-bearing, and the tempdir comes last: Rust drops fields in
/// declaration order, so the [`AppState`] — and with it the manager and its
/// corpus actors — goes first, and the actors are told to stop before their
/// inventories are pulled out from under them.
struct Fixture {
    state: AppState,
    key: String,
    solo_key: String,
    main_cid: String,
    feature_cid: String,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture::build(false)
    }

    /// The same group, with the branch worktree's `opys.toml` corrupted so its
    /// actor never loads — a config edited on a branch, a config removed by a
    /// checkout, a merge left half-resolved. Discovery still finds the corpus,
    /// so the group keeps both members and one of them cannot answer.
    fn broken_branch() -> Fixture {
        Fixture::build(true)
    }

    fn build(break_the_branch: bool) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        // Canonicalized first: discovery canonicalizes worktree roots, and a
        // symlinked TMPDIR would otherwise make every root comparison miss.
        let tmp = std::fs::canonicalize(dir.path()).unwrap();
        let (main, feature) = repo_with_diverged_worktree(&tmp);
        if break_the_branch {
            std::fs::write(feature.join("opys.toml"), "base = = nope\n").unwrap();
        }
        let solo = solo_project(&tmp.join("solo"));

        let config = tmp.join("config/server.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        // Only the main worktree and the solo project are allowlisted; the
        // sibling worktree has to be pulled in by git.
        std::fs::write(
            &config,
            format!(
                "[[project]]\npath = {:?}\n\n[[project]]\npath = {:?}\n",
                main.display().to_string(),
                solo.display().to_string()
            ),
        )
        .unwrap();

        let (events, _) = broadcast::channel(32);
        let mut manager = Manager::new(config, events.clone(), backend);
        manager.rescan().unwrap();

        // Found by membership, never by index: a TMPDIR that itself sits inside
        // a git checkout can bucket unrelated fixtures into extra groups.
        let group_of = |root: &PathBuf| {
            manager
                .groups()
                .iter()
                .find(|g| g.corpora.iter().any(|c| c.root == *root))
                .unwrap_or_else(|| panic!("no group for {}", root.display()))
        };
        let cid_of = |root: &PathBuf| {
            group_of(root)
                .corpora
                .iter()
                .find(|c| c.root == *root)
                .map(|c| c.cid.clone())
                .unwrap_or_else(|| panic!("no corpus for {}", root.display()))
        };
        let key = group_of(&main).key.clone();
        let solo_key = group_of(&solo).key.clone();
        assert_eq!(
            group_of(&main).corpora.len(),
            2,
            "the worktree must be served alongside its project"
        );
        let (main_cid, feature_cid) = (cid_of(&main), cid_of(&feature));

        Fixture {
            state: AppState::new(Arc::new(Mutex::new(manager)), events),
            key,
            solo_key,
            main_cid,
            feature_cid,
            _dir: dir,
        }
    }

    async fn get(&self, uri: &str) -> (StatusCode, Value) {
        let response = api::router(self.state.clone())
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn union(&self, query: &str) -> Value {
        let (status, body) = self
            .get(&format!("/api/group/{}/union{query}", self.key))
            .await;
        assert_eq!(status, StatusCode::OK, "{body:#}");
        body
    }
}

fn ids(view: &Value) -> Vec<&str> {
    view["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| r["id"].as_str().expect("an id"))
        .collect()
}

fn row<'a>(view: &'a Value, id: &str) -> &'a Value {
    view["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("no row for {id}: {:?}", ids(view)))
}

#[tokio::test]
async fn the_union_labels_both_worktrees_and_shows_their_divergence() {
    if !have_git() {
        eprintln!(
            "skipping the_union_labels_both_worktrees_and_shows_their_divergence: git is not on PATH"
        );
        return;
    }
    let fx = Fixture::new();
    let view = fx.union("").await;

    // Columns: the main worktree first, each labelled by its branch.
    let columns = view["columns"].as_array().expect("columns");
    assert_eq!(columns.len(), 2, "{view:#}");
    assert_eq!(columns[0]["cid"], fx.main_cid.as_str());
    assert_eq!(columns[0]["label"], "main (primary)");
    assert_eq!(columns[1]["cid"], fx.feature_cid.as_str());
    assert_eq!(columns[1]["label"], "feature/x");
    assert!(
        columns.iter().all(|c| c.get("error").is_none()),
        "both corpora answered: {view:#}"
    );

    // Rows: the union of both inventories, ordered by the numeric id part.
    assert_eq!(
        ids(&view),
        [
            "NOTE-0001",
            "TASK-0002",
            "TASK-0003",
            "NOTE-0004",
            "TASK-0004"
        ],
        "{view:#}"
    );

    // Identical on both branches.
    let shared = row(&view, "NOTE-0001");
    assert_eq!(shared["title"], "Shared note");
    assert_eq!(shared["differs"], false, "{shared:#}");
    assert_eq!(shared["only_in"], serde_json::json!([]));
    assert_eq!(shared["cells"][0]["status"], "open");
    assert_eq!(shared["cells"][0]["updated"], "2026-01-01T00:00:00Z");
    assert_eq!(shared["cells"][0]["title"], "Shared note");
    assert_eq!(shared["cells"][0]["unknown"], false, "main answered");
    assert_eq!(shared["cells"][1]["status"], "open");
    assert_eq!(shared["cells"][1]["title"], "Shared note");

    // The same document, further along on the branch: the point of the view.
    let drifted = row(&view, "TASK-0002");
    assert_eq!(drifted["differs"], true, "{drifted:#}");
    assert_eq!(drifted["cells"][0]["status"], "doing");
    assert_eq!(drifted["cells"][1]["status"], "done");
    assert_eq!(drifted["only_in"], serde_json::json!([]));

    // Written on the branch and nowhere else.
    let branch_only = row(&view, "TASK-0003");
    assert_eq!(
        branch_only["only_in"],
        serde_json::json!([fx.feature_cid.as_str()])
    );
    assert_eq!(
        branch_only["cells"][0]["status"],
        Value::Null,
        "an absent document has no status"
    );
    assert_eq!(
        branch_only["cells"][0]["unknown"], false,
        "main answered, so the blank cell really is an absence"
    );
    assert_eq!(branch_only["cells"][0]["title"], Value::Null);
    assert_eq!(branch_only["cells"][1]["status"], "todo");
    assert_eq!(
        branch_only["differs"], false,
        "new on a branch is not drift between branches"
    );
    assert_eq!(branch_only["title"], "Branch work", "{branch_only:#}");

    // Two branches, one number: what `opys renumber` exists to repair.
    for (id, cid) in [("NOTE-0004", &fx.main_cid), ("TASK-0004", &fx.feature_cid)] {
        let r = row(&view, id);
        assert_eq!(r["collision"], true, "{r:#}");
        assert_eq!(r["only_in"], serde_json::json!([cid.as_str()]));
    }
    assert_eq!(
        row(&view, "NOTE-0001")["collision"],
        false,
        "an uncontested number is not a collision"
    );
}

/// The filters run per corpus *before* the merge, so a document that matches on
/// one branch and not the other is reported as present on one branch only. That
/// is the specified behaviour and it is worth pinning: it is also the one way
/// the view can mislead, and a client offering the filter has to say so.
#[tokio::test]
async fn filters_apply_per_corpus_before_the_merge() {
    if !have_git() {
        eprintln!("skipping filters_apply_per_corpus_before_the_merge: git is not on PATH");
        return;
    }
    let fx = Fixture::new();

    let view = fx.union("?status=doing").await;
    assert_eq!(ids(&view), ["TASK-0002"], "{view:#}");
    let only = row(&view, "TASK-0002");
    assert_eq!(
        only["only_in"],
        serde_json::json!([fx.main_cid.as_str()]),
        "`done` on the branch was filtered out there, not merged in"
    );
    assert_eq!(
        only["differs"], false,
        "only one column survived the filter"
    );

    let view = fx.union("?type=note").await;
    assert_eq!(ids(&view), ["NOTE-0001", "NOTE-0004"], "{view:#}");
    // The filter hid the branch's TASK-0004, and the impending collision with
    // it is still reported: it is a fact about the corpora, not about the rows
    // the user asked to see.
    assert_eq!(
        row(&view, "NOTE-0004")["collision"],
        true,
        "a filter must not retract the id-collision warning: {view:#}"
    );
}

/// A member that cannot answer is the case the whole `Result` input exists for:
/// its column is labelled with the reason, its cells say "unknown", and no row
/// claims the branch deleted anything.
#[tokio::test]
async fn a_branch_that_cannot_be_read_is_labelled_rather_than_read_as_deleted() {
    if !have_git() {
        eprintln!(
            "skipping a_branch_that_cannot_be_read_is_labelled_rather_than_read_as_deleted: git is not on PATH"
        );
        return;
    }
    let fx = Fixture::broken_branch();
    let view = fx.union("").await;

    let columns = view["columns"].as_array().expect("columns");
    assert_eq!(columns.len(), 2, "the group keeps its shape: {view:#}");
    assert_eq!(columns[0]["cid"], fx.main_cid.as_str());
    assert!(columns[0].get("error").is_none(), "main answered: {view:#}");
    assert_eq!(columns[1]["cid"], fx.feature_cid.as_str());
    let why = columns[1]["error"]
        .as_str()
        .unwrap_or_else(|| panic!("the silent column must say why: {view:#}"));
    assert!(why.contains("not loaded"), "{why}");

    // Only main's documents, since only main spoke.
    assert_eq!(
        ids(&view),
        ["NOTE-0001", "TASK-0002", "NOTE-0004"],
        "{view:#}"
    );
    for id in ["NOTE-0001", "TASK-0002", "NOTE-0004"] {
        let r = row(&view, id);
        assert_eq!(
            r["only_in"],
            serde_json::json!([]),
            "a branch that said nothing is no evidence that {id} is main's alone: {r:#}"
        );
        assert_eq!(r["cells"][0]["unknown"], false);
        assert_eq!(r["cells"][1]["unknown"], true, "{r:#}");
        assert_eq!(
            r["cells"][1]["status"],
            Value::Null,
            "and the blank is not a status"
        );
        assert_eq!(r["differs"], false, "nothing to disagree with: {r:#}");
    }

    // The honest limit: the branch's TASK-0004 is unknown, so the collision
    // with it cannot be seen. The column's error is what says the view is
    // partial — nothing here quietly claims otherwise.
    assert_eq!(row(&view, "NOTE-0004")["collision"], false, "{view:#}");
}

/// A project with no worktrees is not a degenerate case to be refused — it is
/// the common one, and it renders through the same code path.
#[tokio::test]
async fn a_single_corpus_group_is_a_valid_one_column_view() {
    if !have_git() {
        eprintln!("skipping a_single_corpus_group_is_a_valid_one_column_view: git is not on PATH");
        return;
    }
    let fx = Fixture::new();
    let (status, view) = fx.get(&format!("/api/group/{}/union", fx.solo_key)).await;
    assert_eq!(status, StatusCode::OK, "{view:#}");
    assert_eq!(view["columns"].as_array().expect("columns").len(), 1);
    assert_eq!(ids(&view), ["NOTE-0009"], "{view:#}");
    let only = row(&view, "NOTE-0009");
    assert_eq!(only["differs"], false);
    assert_eq!(
        only["only_in"],
        serde_json::json!([]),
        "one column cannot be the only one"
    );
    assert_eq!(only["cells"][0]["status"], "open");
}

#[tokio::test]
async fn an_unknown_group_is_a_json_404() {
    if !have_git() {
        eprintln!("skipping an_unknown_group_is_a_json_404: git is not on PATH");
        return;
    }
    let fx = Fixture::new();
    let (status, body) = fx.get("/api/group/nope/union").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body["error"].as_str().is_some_and(|e| e.contains("nope")),
        "{body:#}"
    );
}
