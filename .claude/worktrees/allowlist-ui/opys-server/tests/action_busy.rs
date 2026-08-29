//! Lock contention on the write path (TASK-0072).
//!
//! Its own test binary, and that is the point: `OPYS_LOCK_TIMEOUT_MS` is
//! process-global, and this file needs a *short* one (a real cycle waits ten
//! seconds by default) while `tests/actions.rs` needs a generous one. One
//! process each is the only way both get what they need without racing.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use opys_backend_markdown_local::MarkdownLocal;
use opys_engine::backend::Backend;
use opys_engine::project::Project;
use opys_server::api::{self, AppState};
use opys_server::manager::Manager;
use serde_json::{json, Value};
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
"#;

fn backend() -> Box<dyn Backend + Send> {
    Box::new(MarkdownLocal)
}

fn project(root: &Path) -> PathBuf {
    let inventory = root.join("inventory");
    std::fs::create_dir_all(&inventory).unwrap();
    std::fs::write(root.join("opys.toml"), CONFIG).unwrap();
    std::fs::write(
        inventory.join("NOTE-0001.md"),
        "---\nid: NOTE-0001\nstatus: open\ntags: [alpha]\n---\n\n# Note one\n\nBody.\n",
    )
    .unwrap();
    std::fs::canonicalize(root).unwrap()
}

/// A node serving one project, with a lock timeout short enough to hit.
fn serve(dir: &Path) -> (AppState, String, PathBuf) {
    std::env::set_var("OPYS_LOCK_TIMEOUT_MS", "200");
    let live = project(&dir.join("live"));
    let config = dir.join("config/server.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        format!("[[project]]\npath = {:?}\n", live.display().to_string()),
    )
    .unwrap();
    let (events, _rx) = broadcast::channel(32);
    let mut manager = Manager::new(config, events.clone(), backend);
    manager.rescan().unwrap();
    let cid = manager.cids().pop().expect("one corpus is served");
    (
        AppState::new(Arc::new(Mutex::new(manager)), events),
        cid,
        live,
    )
}

async fn post_action(state: &AppState, cid: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/corpus/{cid}/action"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = api::router(state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Contention is a **retry**, not a refusal, and it must not describe the
/// server's filesystem while saying so.
///
/// Both halves are load-bearing. A 422 tells a client its write was invalid, so
/// a UI would show "the node is busy" as if the user had asked for something
/// impossible — and contention is partly self-inflicted here, since every action
/// trips the watcher and the actor's reload takes the same flock. And the
/// engine's own message names the inventory directory and the lock file under
/// `$XDG_RUNTIME_DIR`, which is precisely what this endpoint promises never to
/// hand out (ADR-0077): every other payload's paths are relative to the project
/// root.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_inventory_lock_is_a_503_without_paths() {
    let dir = tempfile::tempdir().unwrap();
    let (state, cid, live) = serve(dir.path());
    let before = std::fs::read_to_string(live.join("inventory/NOTE-0001.md")).unwrap();

    // Exactly what a concurrent `opys` invocation holds, and for longer than the
    // cycle is willing to wait.
    let prj = Project::open(&live.to_string_lossy()).unwrap();
    let (mut store, _errors) = MarkdownLocal.load(&prj).expect("the test takes the lock");

    let (status, answer) = post_action(
        &state,
        &cid,
        json!({"action": "tag", "id": "NOTE-0001", "add": "blocked-on-the-lock"}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{answer}");
    let message = answer["error"].as_str().expect("a message");
    assert!(
        !message.contains('/'),
        "the reply must not describe the server's filesystem: {message}"
    );
    assert_eq!(
        std::fs::read_to_string(live.join("inventory/NOTE-0001.md")).unwrap(),
        before,
        "a refused-for-busy write must not have touched anything"
    );

    // …and the same request works once the other writer is done, which is what
    // makes 503 (retry) the honest answer rather than 422 (do not).
    drop(store.take_lock());
    drop(store);
    let (status, answer) = post_action(
        &state,
        &cid,
        json!({"action": "tag", "id": "NOTE-0001", "add": "after-the-lock"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert!(
        std::fs::read_to_string(live.join("inventory/NOTE-0001.md"))
            .unwrap()
            .contains("after-the-lock"),
        "the retry landed"
    );
}
