//! Main remains the pick oracle; branch and worktree states are labeled progress.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
#[cfg(feature = "git")]
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use opys_backend_markdown_local::MarkdownLocal;
use opys_engine::backend::Backend;
use opys_server::api::{self, AppState};
use opys_server::flow;
use opys_server::manager::Manager;
use opys_server::union::{Cell, Column, ColumnKind, Row, UnionView};
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tower::ServiceExt;

const CONFIG: &str = r#"
base = "inventory"
[types.task]
prefix = "TASK"
statuses = ["todo", "doing", "review", "done"]
default_status = "todo"
terminal_statuses = ["done"]
tags_required = false
[types.task.transitions]
todo = ["doing"]
doing = ["review", "todo"]
review = ["done", "doing"]
[types.note]
prefix = "NOTE"
statuses = ["open"]
default_status = "open"
tags_required = false
"#;

fn have_git() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "init.defaultBranch=main",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn doc(root: &Path, id: &str, status: &str, title: &str, extra: &str) {
    std::fs::write(
        root.join("inventory").join(format!("{id}.md")),
        format!("---\nid: {id}\nstatus: {status}\n{extra}---\n\n# {title}\n\nBody.\n"),
    )
    .unwrap();
}

fn backend() -> Box<dyn Backend + Send> {
    Box::new(MarkdownLocal)
}

struct Fixture {
    state: AppState,
    manager: Arc<Mutex<Manager>>,
    key: String,
    main_cid: String,
    worktree_cid: String,
    worktree: PathBuf,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn new(broken_worktree: bool, unchecked_branch: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("main");
        std::fs::create_dir_all(root.join("inventory")).unwrap();
        std::fs::write(root.join("opys.toml"), CONFIG).unwrap();
        doc(
            &root,
            "TASK-0001",
            "todo",
            "Shared task",
            "blocks:\n  TASK-0002: Blocked main\n",
        );
        doc(
            &root,
            "TASK-0002",
            "doing",
            "Blocked main",
            "blocked_by:\n  TASK-0001: Shared task\n",
        );
        doc(&root, "NOTE-0003", "open", "Unchanged", "");
        git(&root, &["init"]);
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-m", "seed main"]);
        if unchecked_branch {
            git(&root, &["checkout", "-b", "feature/uncheckout"]);
            doc(
                &root,
                "TASK-0001",
                "review",
                "Shared task",
                "blocks:\n  TASK-0002: Blocked main\n",
            );
            doc(&root, "TASK-0004", "doing", "Only on unchecked branch", "");
            git(&root, &["add", "-A"]);
            git(&root, &["commit", "-m", "unchecked progress"]);
            git(&root, &["checkout", "main"]);
        }
        let worktree = dir.path().join("worktree");
        git(
            &root,
            &[
                "worktree",
                "add",
                "-b",
                "feature/worktree",
                worktree.to_str().unwrap(),
            ],
        );
        doc(
            &worktree,
            "TASK-0001",
            "doing",
            "Shared task",
            "blocks:\n  TASK-0002: Blocked main\n",
        );
        doc(
            &worktree,
            "TASK-0002",
            "doing",
            "Renamed on live worktree",
            "blocked_by:\n  TASK-0001: Shared task\n",
        );
        doc(
            &worktree,
            "TASK-0005",
            "review",
            "Only in live worktree",
            "",
        );
        if broken_worktree {
            std::fs::write(worktree.join("opys.toml"), "base = = broken\n").unwrap();
        }
        let root = std::fs::canonicalize(root).unwrap();
        let worktree = std::fs::canonicalize(worktree).unwrap();
        let registry = dir.path().join("server.toml");
        std::fs::write(
            &registry,
            format!("[[project]]\npath = {:?}\n", root.display().to_string()),
        )
        .unwrap();
        let (events, _) = broadcast::channel(32);
        let mut manager = Manager::new(registry, events.clone(), backend);
        manager.rescan().unwrap();
        let group = manager
            .groups()
            .iter()
            .find(|group| group.corpora.iter().any(|corpus| corpus.root == root))
            .unwrap();
        let key = group.key.clone();
        let cid = |path: &Path| {
            group
                .corpora
                .iter()
                .find(|corpus| corpus.root == path)
                .unwrap()
                .cid
                .clone()
        };
        let main_cid = cid(&root);
        let worktree_cid = cid(&worktree);
        let manager = Arc::new(Mutex::new(manager));
        Self {
            state: AppState::new(Arc::clone(&manager), events),
            manager,
            key,
            main_cid,
            worktree_cid,
            worktree,
            _dir: dir,
        }
    }

    async fn get(&self, path: &str) -> Value {
        let response = api::router(self.state.clone())
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status, StatusCode::OK, "{value:#}");
        value
    }

    async fn flow(&self) -> Value {
        self.get(&format!("/api/projects/{}/flow?n=1", self.key))
            .await
    }

    #[cfg(feature = "git")]
    async fn wait_for_branch(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if self
                .manager
                .lock()
                .unwrap()
                .branches_for(&self.key)
                .columns
                .len()
                == 1
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "unchecked branch never became a source"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

fn row<'a>(progress: &'a Value, id: &str) -> &'a Value {
    progress["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id)
        .unwrap_or_else(|| panic!("missing {id}: {progress:#}"))
}

#[tokio::test]
async fn live_worktree_progress_keeps_main_pickability_and_source_identity() {
    if !have_git() {
        return;
    }
    let fixture = Fixture::new(false, false);
    let flow = fixture.flow().await;
    assert_eq!(flow["cid"], fixture.main_cid);
    for node in flow["nodes"].as_array().unwrap() {
        let status = node["status"].as_str().unwrap();
        let next = fixture
            .get(&format!(
                "/api/projects/{}/next?status={status}&n=1",
                fixture.key
            ))
            .await;
        assert_eq!(
            node["items"], next["items"],
            "{status}: main's oracle alone supplies node items"
        );
        assert_eq!(node["pickable"], next["total"]);
        assert_eq!(node["orphaned"], next["orphaned"]);
    }
    let progress = &flow["progress"];
    assert_eq!(progress["columns"].as_array().unwrap().len(), 2);
    assert!(progress["rows"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["id"] != "NOTE-0003"));
    let shared = row(progress, "TASK-0001");
    assert_eq!(shared["cells"][0]["status"], "todo");
    assert_eq!(shared["cells"][1]["status"], "doing");
    assert_eq!(shared["cells"][1]["cid"], fixture.worktree_cid);
    assert_eq!(
        row(progress, "TASK-0002")["cells"][1]["title"],
        "Renamed on live worktree"
    );
    let only = row(progress, "TASK-0005");
    assert_eq!(only["cells"][0]["status"], Value::Null);
    assert_eq!(only["cells"][0]["unknown"], false);
    assert_eq!(only["cells"][1]["status"], "review");
    assert_eq!(only["only_in"], json!([fixture.worktree_cid]));
    for row in progress["rows"].as_array().unwrap() {
        assert_eq!(
            row["cells"].as_array().unwrap().len(),
            progress["columns"].as_array().unwrap().len()
        );
    }
    doc(
        &fixture.worktree,
        "TASK-0001",
        "review",
        "Latest unsaved working state",
        "blocks:\n  TASK-0002: Blocked main\n",
    );
    let client = fixture
        .manager
        .lock()
        .unwrap()
        .get(&fixture.worktree_cid)
        .unwrap()
        .client();
    tokio::task::spawn_blocking(move || client.reload())
        .await
        .unwrap()
        .unwrap();
    let latest = fixture.flow().await;
    assert_eq!(
        row(&latest["progress"], "TASK-0001")["cells"][1]["status"],
        "review"
    );
    assert_eq!(
        row(&latest["progress"], "TASK-0001")["cells"][1]["title"],
        "Latest unsaved working state"
    );
    assert_eq!(
        latest["nodes"], flow["nodes"],
        "a worktree edit never changes main's graph counts or items"
    );
}

#[cfg(feature = "git")]
#[tokio::test]
async fn unchecked_branches_keep_multiple_states_and_branch_only_documents() {
    if !have_git() {
        return;
    }
    let fixture = Fixture::new(false, true);
    fixture.wait_for_branch().await;
    let flow = fixture.flow().await;
    let progress = &flow["progress"];
    let columns = progress["columns"].as_array().unwrap();
    assert_eq!(
        columns.len(),
        3,
        "a served worktree is not duplicated as a ref: {progress:#}"
    );
    assert_eq!(columns[2]["kind"], "branch");
    assert_eq!(columns[2]["label"], "feature/uncheckout");
    assert_eq!(
        row(progress, "TASK-0001")["cells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|cell| cell["status"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["todo", "doing", "review"]
    );
    let only = row(progress, "TASK-0004");
    assert_eq!(only["cells"][0]["status"], Value::Null);
    assert_eq!(only["cells"][1]["status"], Value::Null);
    assert_eq!(only["cells"][2]["status"], "doing");
    assert_eq!(only["only_in"], json!([columns[2]["cid"]]));
    assert!(flow["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|node| node["items"].as_array().unwrap())
        .all(|item| item["id"] != "TASK-0004"));
}

#[tokio::test]
async fn failed_worktrees_remain_errors_and_never_look_deleted() {
    if !have_git() {
        return;
    }
    let fixture = Fixture::new(true, false);
    let flow = fixture.flow().await;
    let progress = &flow["progress"];
    assert_eq!(progress["columns"][1]["cid"], fixture.worktree_cid);
    assert!(progress["columns"][1]["error"].as_str().is_some());
    assert_eq!(
        progress["rows"],
        json!([]),
        "an unreadable worktree supplies no claimed progress"
    );
    assert!(flow["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["status"] == "todo")
        .unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == "TASK-0001"));
}

fn column(cid: &str, error: Option<&str>) -> Column {
    Column {
        cid: cid.into(),
        label: cid.into(),
        error: error.map(str::to_string),
        kind: ColumnKind::Worktree,
        commit: None,
    }
}

fn cell(cid: &str, status: Option<&str>, unknown: bool) -> Cell {
    Cell {
        cid: cid.into(),
        status: status.map(str::to_string),
        title: status.map(|_| "Task".into()),
        updated: None,
        unknown,
    }
}

#[test]
fn progress_preserves_notices_and_unknown_primary_cells() {
    let view = UnionView {
        columns: vec![column("main", Some("unavailable")), column("branch", None)],
        rows: vec![Row {
            id: "TASK-0001".into(),
            title: "Task".into(),
            cells: vec![
                cell("main", None, true),
                cell("branch", Some("doing"), false),
            ],
            differs: false,
            only_in: Vec::new(),
            collision: false,
            occupants: Vec::new(),
        }],
        notices: vec!["branch reader could not inspect one source".into()],
    };
    let progress = flow::progress(view, "main").unwrap();
    assert_eq!(
        progress.notices,
        ["branch reader could not inspect one source"]
    );
    assert_eq!(progress.columns[0].error.as_deref(), Some("unavailable"));
    assert!(progress.rows[0].cells[0].unknown);
    assert!(
        progress.rows[0].only_in.is_empty(),
        "unknown main cannot establish a branch-only document"
    );
    let notices_only = UnionView {
        columns: vec![column("main", None)],
        rows: Vec::new(),
        notices: vec!["not a git repository".into()],
    };
    assert!(flow::progress(notices_only, "main").is_some());
    let no_progress = UnionView {
        columns: vec![column("main", None)],
        rows: Vec::new(),
        notices: Vec::new(),
    };
    assert!(flow::progress(no_progress, "main").is_none());
}

#[test]
fn failed_unchecked_sources_keep_errors_after_main_only_rows_are_removed() {
    let mut failed = column("ref-failed", Some("branch inventory could not be parsed"));
    failed.kind = ColumnKind::Branch;
    failed.commit = Some("a".repeat(40));
    let view = UnionView {
        columns: vec![column("main", None), failed],
        rows: vec![Row {
            id: "TASK-0001".into(),
            title: "Task".into(),
            cells: vec![
                cell("main", Some("todo"), false),
                cell("ref-failed", None, true),
            ],
            differs: false,
            only_in: Vec::new(),
            collision: false,
            occupants: Vec::new(),
        }],
        notices: Vec::new(),
    };
    let progress = flow::progress(view, "main").unwrap();
    assert!(progress.rows.is_empty());
    assert_eq!(progress.columns.len(), 2);
    assert_eq!(progress.columns[1].kind, ColumnKind::Branch);
    assert_eq!(
        progress.columns[1].error.as_deref(),
        Some("branch inventory could not be parsed")
    );
}
