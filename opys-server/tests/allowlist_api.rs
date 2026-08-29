//! Managing the allowlist from the browser (ADR-0082, FEAT-0083).
//!
//! The rules live in `registry::vet_ui_path` and are tested exhaustively in
//! `vet_path.rs`. What is tested here is that the *endpoints* apply them — that
//! there is no route into the allowlist that skips the vetting — plus the
//! onboarding state machine and the paths-only suggestion contract.

use std::path::Path;
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

fn project_at(dir: &Path) {
    std::fs::create_dir_all(dir.join("inventory")).unwrap();
    std::fs::write(dir.join("opys.toml"), CONFIG).unwrap();
}

/// `HOME` is process-wide and these tests all depend on it, so each one holds a
/// lock for its whole body — across awaits, which is why the mutex is tokio's.
///
/// Restoring on drop rather than at the end of a helper matters: a failing
/// assertion unwinds, and a test that left `HOME` pointing at a deleted tempdir
/// would take the rest of the file down with it.
static HOME_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct HomeGuard {
    _guard: tokio::sync::MutexGuard<'static, ()>,
    prev: Option<std::ffi::OsString>,
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}

async fn set_home(home: &Path) -> HomeGuard {
    let guard = HOME_LOCK.lock().await;
    let prev = std::env::var_os("HOME");
    unsafe { std::env::set_var("HOME", home) };
    HomeGuard {
        _guard: guard,
        prev,
    }
}

struct Fx {
    state: AppState,
    config: std::path::PathBuf,
}

impl Fx {
    /// A node whose allowlist file does not exist yet — the onboarding case.
    fn unconfigured(home: &Path) -> Fx {
        let config = home.join(".config/opys/server.toml");
        let (events, _) = broadcast::channel(32);
        let manager = Manager::new(config.clone(), events.clone(), backend);
        Fx {
            state: AppState::new(Arc::new(Mutex::new(manager)), events),
            config,
        }
    }

    async fn send(&self, r: Request<Body>) -> (StatusCode, Value) {
        let res = api::router(self.state.clone()).oneshot(r).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn get(&self, uri: &str) -> (StatusCode, Value) {
        self.send(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
    }

    async fn post(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.send(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
    }
}

/// A home with two projects under `Projects/`, plus a hidden one.
fn home_with_projects() -> tempfile::TempDir {
    let h = tempfile::tempdir().unwrap();
    project_at(&h.path().join("Projects/alpha"));
    project_at(&h.path().join("Projects/beta"));
    project_at(&h.path().join(".hidden/sneaky"));
    h
}

#[tokio::test]
async fn a_missing_allowlist_is_what_triggers_onboarding() {
    let h = home_with_projects();
    let _home = set_home(h.path()).await;
    let fx = Fx::unconfigured(h.path());
    let (status, body) = fx.get("/api/setup").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["configured"], false, "{body}");
    assert_eq!(body["mode"], "suggest", "the default mode");
    assert!(
        body["scan_root"].as_str().unwrap().ends_with("Projects"),
        "a home with Projects/ narrows the default root: {body}"
    );
}

/// An *empty* allowlist is a decision already made; it must not re-onboard.
#[tokio::test]
async fn an_empty_allowlist_file_counts_as_configured() {
    let h = home_with_projects();
    let _home = set_home(h.path()).await;
    let fx = Fx::unconfigured(h.path());
    std::fs::create_dir_all(fx.config.parent().unwrap()).unwrap();
    std::fs::write(&fx.config, "mode = \"off\"\n").unwrap();
    let (_, body) = fx.get("/api/setup").await;
    assert_eq!(body["configured"], true, "{body}");
    assert_eq!(body["mode"], "off", "{body}");
}

#[tokio::test]
async fn setup_writes_the_mode_and_reports_it_back() {
    let h = home_with_projects();
    let _home = set_home(h.path()).await;
    let fx = Fx::unconfigured(h.path());
    let (status, body) = fx.post("/api/setup", json!({"mode": "off"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["mode"], "off");
    assert_eq!(body["configured"], true);
    assert!(fx.config.exists(), "the file was written");

    let (status, body) = fx.post("/api/setup", json!({"mode": "auto-add"})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("auto-add"),
        "the refusal names what was asked for: {body}"
    );
}

#[tokio::test]
async fn a_project_can_be_added_and_removed_from_the_browser() {
    let h = home_with_projects();
    let _home = set_home(h.path()).await;
    let fx = Fx::unconfigured(h.path());
    let alpha = h.path().join("Projects/alpha").display().to_string();

    let (status, body) = fx
        .post("/api/allowlist", json!({"action": "add", "path": alpha}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["entries"].as_array().unwrap().len(), 1, "{body}");

    let written = std::fs::read_to_string(&fx.config).unwrap();
    assert!(written.contains("[[project]]"), "{written}");

    let (status, body) = fx
        .post("/api/allowlist", json!({"action": "remove", "path": alpha}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["entries"].as_array().unwrap().is_empty(), "{body}");
}

/// The point of the whole feature: the endpoint cannot be talked into a path
/// the rules forbid. `vet_path.rs` proves the rules; this proves they are on.
#[tokio::test]
async fn the_endpoint_refuses_what_the_rules_refuse() {
    let h = home_with_projects();
    let _home = set_home(h.path()).await;
    let outside = tempfile::tempdir().unwrap();
    project_at(outside.path());
    let fx = Fx::unconfigured(h.path());

    for (raw, expect) in [
        (outside.path().display().to_string(), "outside your home"),
        (
            h.path().join(".hidden/sneaky").display().to_string(),
            "hidden directory",
        ),
        (
            h.path().join("Projects/../..").display().to_string(),
            "outside your home",
        ),
    ] {
        let (status, body) = fx
            .post("/api/allowlist", json!({"action": "add", "path": raw}))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{raw}: {body}");
        let msg = body["error"].as_str().unwrap_or_default();
        assert!(
            msg.contains(expect),
            "{raw}: expected {expect:?}, got {msg}"
        );
        assert!(
            !fx.config.exists()
                || !std::fs::read_to_string(&fx.config)
                    .unwrap()
                    .contains("[[project]]"),
            "a refused add must write nothing"
        );
    }
}

/// A suggestion is a path and nothing more — no doc count, no verify dot, no
/// title. Rendering any of those means opening the project.
#[tokio::test]
async fn suggestions_are_paths_only_and_exclude_what_is_allowlisted() {
    let h = home_with_projects();
    let _home = set_home(h.path()).await;
    let fx = Fx::unconfigured(h.path());

    let (status, body) = fx.get("/api/suggestions").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body.as_array().unwrap();
    assert_eq!(items.len(), 2, "alpha and beta, not the hidden one: {body}");
    for item in items {
        let mut keys: Vec<&str> = item
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["already_allowlisted", "name", "path"],
            "a suggestion carries nothing read from inside the project: {item}"
        );
    }

    // Allowlisting one drops it from the suggestions.
    let alpha = h.path().join("Projects/alpha").display().to_string();
    fx.post("/api/allowlist", json!({"action": "add", "path": alpha}))
        .await;
    let (_, body) = fx.get("/api/suggestions").await;
    assert_eq!(body.as_array().unwrap().len(), 1, "{body}");
}

#[tokio::test]
async fn mode_off_suggests_nothing() {
    let h = home_with_projects();
    let _home = set_home(h.path()).await;
    let fx = Fx::unconfigured(h.path());
    fx.post("/api/setup", json!({"mode": "off"})).await;
    let (status, body) = fx.get("/api/suggestions").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.as_array().unwrap().is_empty(), "{body}");
}

/// A config predating this setting must read as `suggest` — the behaviour it
/// already had. A silent switch to `off` would stop a working node scanning.
#[tokio::test]
async fn a_config_without_a_mode_key_reads_as_suggest() {
    let h = home_with_projects();
    let _home = set_home(h.path()).await;
    let fx = Fx::unconfigured(h.path());
    std::fs::create_dir_all(fx.config.parent().unwrap()).unwrap();
    std::fs::write(&fx.config, "bind = \"127.0.0.1:6797\"\n").unwrap();

    let (_, body) = fx.get("/api/setup").await;
    assert_eq!(body["mode"], "suggest", "{body}");
    let (_, body) = fx.get("/api/suggestions").await;
    assert_eq!(body.as_array().unwrap().len(), 2, "it still scans: {body}");
}

/// The UI's rules bind the UI, not the file. An entry someone added by hand from
/// outside `$HOME` keeps working, is shown, and can still be removed — the
/// alternative is a screen that hides entries it did not create, or worse,
/// rewrites the file to drop them.
#[tokio::test]
async fn an_outside_home_entry_added_by_hand_is_kept_and_removable() {
    let h = home_with_projects();
    let outside = tempfile::tempdir().unwrap();
    project_at(outside.path());
    let _home = set_home(h.path()).await;
    let fx = Fx::unconfigured(h.path());
    std::fs::create_dir_all(fx.config.parent().unwrap()).unwrap();
    std::fs::write(
        &fx.config,
        format!("[[project]]\npath = \"{}\"\n", outside.path().display()),
    )
    .unwrap();

    let (_, body) = fx.get("/api/setup").await;
    assert_eq!(
        body["entries"].as_array().unwrap().len(),
        1,
        "shown, not hidden: {body}"
    );

    // Adding a project alongside it must not disturb it.
    let alpha = h.path().join("Projects/alpha").display().to_string();
    let (status, body) = fx
        .post("/api/allowlist", json!({"action": "add", "path": alpha}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let written = std::fs::read_to_string(&fx.config).unwrap();
    assert!(
        written.contains(&outside.path().display().to_string()),
        "the hand-written entry survived the edit: {written}"
    );

    // And it can be removed from the UI that is showing it, even though `add`
    // would have refused the same path.
    let (status, body) = fx
        .post(
            "/api/allowlist",
            json!({"action": "remove", "path": outside.path().display().to_string()}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["entries"].as_array().unwrap().len(), 1, "{body}");
}
