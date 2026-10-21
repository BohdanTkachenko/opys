//! Declared stages, claims and safe branch writes through the real HTTP router.
#![cfg(feature = "git")]

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
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tower::ServiceExt;

const CONFIG: &str = r#"
base = "inventory"
[roles.dan]
description = "Approval"
[roles.agent]
description = "Research"
[types.task]
prefix = "TASK"
statuses = ["todo", "research", "approval", "ready", "implement", "done"]
default_status = "todo"
terminal_statuses = ["done"]
tags_required = false
claim_field = "claimed_by"
claim_ttl = "12h"
releases_blocks = ["approval", "done"]
[types.task.owners]
research = ["agent"]
approval = ["dan"]
[types.task.fields.claimed_by]
type = "string"
[types.task.work_branches.spec]
ref = "spec/{id}"
from = "todo"
stages = ["research", "approval"]
merged = "ready"
[types.task.work_branches.impl]
ref = "impl/{id}"
from = "ready"
stages = ["implement"]
merged = "done"
"#;

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
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

fn doc(root: &Path, status: &str, claim: bool) {
    let claim = if claim { "claimed_by: worker-7\n" } else { "" };
    std::fs::write(root.join("inventory/TASK-0001.md"), format!(
        "---\nid: TASK-0001\nstatus: {status}\nupdated: '2026-10-20T12:00:00Z'\n{claim}---\n\n# {status} title\n\n{status} prose.\n"
    )).unwrap();
}

fn backend() -> Box<dyn Backend + Send> {
    Box::new(MarkdownLocal)
}

struct Fixture {
    state: AppState,
    manager: Arc<Mutex<Manager>>,
    root: PathBuf,
    checkout: PathBuf,
    key: String,
    cid: String,
    branch_cid: Option<String>,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn new(claim: bool, checkout: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("main");
        std::fs::create_dir_all(root.join("inventory")).unwrap();
        std::fs::write(root.join("opys.toml"), CONFIG).unwrap();
        doc(&root, "todo", false);
        std::fs::write(root.join("inventory/TASK-0002.md"), "---\nid: TASK-0002\nstatus: ready\nblocked_by:\n  TASK-0001: Blocker\n---\n\n# Blocked on main\n").unwrap();
        git(&root, &["init", "-b", "main"]);
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "main"]);
        git(&root, &["switch", "-c", "spec/TASK-0001"]);
        doc(&root, "approval", claim);
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "spec"]);
        git(&root, &["switch", "main"]);
        let branch = dir.path().join("branch");
        if checkout {
            git(
                &root,
                &[
                    "worktree",
                    "add",
                    branch.to_str().unwrap(),
                    "spec/TASK-0001",
                ],
            );
        }
        let registry = dir.path().join("server.toml");
        std::fs::write(
            &registry,
            format!("[[project]]\npath = {:?}\n", root.to_str().unwrap()),
        )
        .unwrap();
        let (events, _) = broadcast::channel(128);
        let mut manager = Manager::new(registry, events.clone(), backend);
        manager.rescan().unwrap();
        let group = manager
            .groups()
            .iter()
            .find(|g| g.corpora.iter().any(|c| c.root == root))
            .unwrap();
        let key = group.key.clone();
        let cid = group
            .corpora
            .iter()
            .find(|c| c.root == root)
            .unwrap()
            .cid
            .clone();
        let branch_cid = group
            .corpora
            .iter()
            .find(|c| c.root == branch)
            .map(|c| c.cid.clone());
        let manager = Arc::new(Mutex::new(manager));
        Self {
            state: AppState::new(Arc::clone(&manager), events),
            manager,
            root,
            checkout: branch,
            key,
            cid,
            branch_cid,
            _dir: dir,
        }
    }

    async fn send(&self, path: &str, action: Option<Value>) -> (StatusCode, Value) {
        let mut request = Request::builder().uri(path);
        let body = if let Some(action) = action {
            request = request
                .method("POST")
                .header("Content-Type", "application/json");
            Body::from(action.to_string())
        } else {
            Body::empty()
        };
        let response = api::router(self.state.clone())
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn get(&self, path: &str) -> Value {
        let (status, value) = self.send(path, None).await;
        assert_eq!(status, StatusCode::OK, "{value:#}");
        value
    }

    async fn flow(&self) -> Value {
        self.get(&format!("/api/projects/{}/flow", self.key)).await
    }

    async fn wait_flow(&self, ready: impl Fn(&Value) -> bool) -> Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let flow = self.flow().await;
            if ready(&flow) {
                return flow;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "background work projection did not refresh: {flow:#}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
}

fn node<'a>(flow: &'a Value, status: &str) -> &'a Value {
    flow["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["status"] == status)
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn flow_role_and_document_use_branch_stage_and_claim_with_main_blockers() {
    let fx = Fixture::new(true, false);
    let flow = fx.flow().await;
    assert_eq!(node(&flow, "todo")["total"], 0);
    let approval = node(&flow, "approval");
    assert_eq!(approval["total"], 1);
    assert_eq!(approval["pickable"], 0);
    assert_eq!(approval["in_progress"][0]["ref"], "spec/TASK-0001");
    assert_eq!(approval["in_progress"][0]["claim"]["holder"], "worker-7");
    assert_eq!(
        node(&flow, "ready")["blocked"],
        json!(["TASK-0002"]),
        "a branch's releasing stage cannot release a main blocker"
    );
    let role = fx.get("/api/roles/dan").await;
    let group = role["projects"][0]["queue"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["status"] == "approval")
        .unwrap();
    assert_eq!(group["total"], 0);
    assert_eq!(group["in_progress"][0]["ref"], "spec/TASK-0001");
    let doc = fx
        .get(&format!("/api/corpus/{}/doc/TASK-0001", fx.cid))
        .await;
    assert_eq!(doc["status"], "approval");
    assert!(doc["body"].as_str().unwrap().contains("approval prose"));
    assert_eq!(doc["work"]["ref"], "spec/TASK-0001");
    assert!(doc["work"]["read_only_reason"]
        .as_str()
        .unwrap()
        .contains("served checkout"));
    assert!(doc["work"]["edit_cid"].is_null());
    assert_eq!(doc["closable"], false);
    assert!(!doc["allowed_targets"]
        .as_array()
        .unwrap()
        .contains(&json!("ready")));
}

#[tokio::test(flavor = "multi_thread")]
async fn occupied_main_rejects_every_targeted_write_and_relation_side_effect() {
    let fx = Fixture::new(false, false);
    let path = fx.root.join("inventory/TASK-0001.md");
    let before = std::fs::read(&path).unwrap();
    for action in [
        json!({"action":"set-status", "id":"TASK-0001", "status":"research"}),
        json!({"action":"edit-body", "id":"TASK-0001", "body":"# Overwrite"}),
        json!({"action":"set-field", "id":"TASK-0001", "key":"claimed_by", "value":"other"}),
        json!({"action":"remove-field", "id":"TASK-0001", "key":"claimed_by"}),
        json!({"action":"apply", "id":"TASK-0001", "log":"Bad main write"}),
        json!({"action":"tag", "id":"TASK-0001", "add":"oops"}),
        json!({"action":"log", "id":"TASK-0001", "text":"Oops"}),
        json!({"action":"close", "id":"TASK-0001"}),
        json!({"action":"verify-fix", "id":"TASK-0001"}),
        json!({"action":"block", "id":"TASK-0002", "by":"TASK-0001"}),
        json!({"action":"unblock", "id":"TASK-0002", "by":"TASK-0001"}),
    ] {
        let (status, body) = fx
            .send(&format!("/api/corpus/{}/action", fx.cid), Some(action))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#}");
        assert!(
            body["error"].as_str().unwrap().contains("spec/TASK-0001"),
            "{body:#}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn served_checkout_routes_edits_to_local_branch_files_and_refuses_completion() {
    let fx = Fixture::new(false, true);
    let cid = fx.branch_cid.as_ref().expect("worktree must be served");
    let main_path = fx.root.join("inventory/TASK-0001.md");
    let main_before = std::fs::read(&main_path).unwrap();
    let main_doc = fx
        .get(&format!("/api/corpus/{}/doc/TASK-0001", fx.cid))
        .await;
    assert_eq!(main_doc["work"]["edit_cid"], cid.as_str());
    let branch_doc = fx.get(&format!("/api/corpus/{cid}/doc/TASK-0001")).await;
    assert!(
        branch_doc["work"]["read_only_reason"].is_null(),
        "{branch_doc:#}"
    );
    let (status, body) = fx
        .send(
            &format!("/api/corpus/{cid}/action"),
            Some(json!({
                "action":"edit-body", "id":"TASK-0001", "body":"# Edited branch\n\nLocal draft.\n"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body:#}");
    assert!(
        std::fs::read_to_string(fx.checkout.join("inventory/TASK-0001.md"))
            .unwrap()
            .contains("Local draft")
    );
    assert_eq!(std::fs::read(&main_path).unwrap(), main_before);
    let fresh = fx.get(&format!("/api/corpus/{cid}/doc/TASK-0001")).await;
    assert!(
        fresh["body"].as_str().unwrap().contains("Local draft"),
        "uncommitted checkout edits must remain visible"
    );
    for action in [
        json!({"action":"set-status", "id":"TASK-0001", "status":"ready"}),
        json!({"action":"close", "id":"TASK-0001"}),
    ] {
        let (status, body) = fx
            .send(&format!("/api/corpus/{cid}/action"), Some(action))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#}");
        assert!(
            body["error"].as_str().unwrap().contains("orchestrator"),
            "{body:#}"
        );
    }
    git(&fx.root, &["branch", "impl/TASK-0001", "spec/TASK-0001"]);
    fx.wait_flow(|f| node(f, "todo")["held"] == json!(["TASK-0001"]))
        .await;
    let unknown = fx
        .get(&format!("/api/corpus/{}/doc/TASK-0001", fx.cid))
        .await;
    assert!(unknown["work"]["read_only_reason"]
        .as_str()
        .unwrap()
        .contains("two kinds"));
    assert!(
        unknown["work"]["edit_cid"].is_null(),
        "an unavailable copy must not offer an edit route"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn moved_refs_refresh_in_background_and_conflicting_kinds_are_held() {
    let fx = Fixture::new(false, false);
    assert_eq!(node(&fx.flow().await, "approval")["pickable"], 1);
    git(&fx.root, &["switch", "spec/TASK-0001"]);
    doc(&fx.root, "research", true);
    git(&fx.root, &["add", "."]);
    git(&fx.root, &["commit", "-m", "research claim"]);
    git(&fx.root, &["switch", "main"]);
    // No explicit reload: the ref reader refreshes the background projection.
    let flow = fx
        .wait_flow(|f| node(f, "research")["in_progress"][0]["claim"]["holder"] == "worker-7")
        .await;
    assert_eq!(node(&flow, "approval")["total"], 0);
    assert_eq!(
        node(&flow, "research")["in_progress"][0]["claim"]["holder"],
        "worker-7"
    );
    git(&fx.root, &["branch", "impl/TASK-0001", "spec/TASK-0001"]);
    let flow = fx
        .wait_flow(|f| node(f, "todo")["held"] == json!(["TASK-0001"]))
        .await;
    assert_eq!(node(&flow, "todo")["pickable"], 0);
    assert_eq!(node(&flow, "todo")["held"], json!(["TASK-0001"]));
    assert!(flow["notices"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n.as_str().unwrap().contains("two kinds")));
    let doc = fx
        .get(&format!("/api/corpus/{}/doc/TASK-0001", fx.cid))
        .await;
    assert!(doc["work"]["read_only_reason"]
        .as_str()
        .unwrap()
        .contains("two kinds"));
    assert!(fx.manager.lock().unwrap().get(&fx.cid).is_some());
}
