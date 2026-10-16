//! The read API over real corpora, driven through the router rather than a
//! socket.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use clap::Parser;
use http_body_util::BodyExt;
use opys_backend_markdown_local::MarkdownLocal;
use opys_engine::backend::Backend;
use opys_engine::cli::Cli;
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

[types.task]
prefix = "TASK"
statuses = ["todo", "doing", "done"]
default_status = "todo"
terminal_statuses = ["done"]
tags_required = false

# A status graph on one type, so the doc payload has to distinguish "this type
# declares no graph" from "these are the moves" (`note` declares none).
[types.task.transitions]
todo = ["doing"]
doing = ["todo", "done"]

[types.task.fields.priority]
type = "int"

[[types.task.sections]]
heading = "Progress"
kind = "log"

# The shape the shipped `feature` type has, and this repo's own `feature` and
# `adr`: a status graph and *no* terminal status. `closable` is false for every
# document of it, and not because of the graph — the payload has to say which of
# the two refusals applies, or a client reads "not from here yet" into "never".
[types.idea]
prefix = "IDEA"
statuses = ["draft", "live"]
default_status = "draft"
terminal_statuses = []
tags_required = false

[types.idea.transitions]
draft = ["live"]
live = ["draft"]
"#;

fn backend() -> Box<dyn Backend + Send> {
    Box::new(MarkdownLocal)
}

/// A project with two linked documents of two types, so the filters and the
/// relation maps both have something to say, plus a third of the graph-without-
/// a-terminal-status type nothing else covers.
fn project(root: &Path) -> PathBuf {
    let inventory = root.join("inventory");
    std::fs::create_dir_all(&inventory).unwrap();
    std::fs::write(root.join("opys.toml"), CONFIG).unwrap();
    std::fs::write(
        inventory.join("NOTE-0001.md"),
        "---\n\
         id: NOTE-0001\n\
         status: open\n\
         tags: [alpha, shared]\n\
         updated: \"2026-01-01T00:00:00Z\"\n\
         blocks:\n  TASK-0002: Do the thing\n\
         history:\n  \"2026-01-01T00:00:00.000000Z\": open\n\
         references:\n  TASK-0002: Do the thing\n\
         ---\n\n# Note one\n\nHello <script>alert(1)</script> world.\n",
    )
    .unwrap();
    std::fs::write(
        inventory.join("TASK-0002.md"),
        "---\n\
         id: TASK-0002\n\
         status: doing\n\
         tags: [shared]\n\
         blocked_by:\n  NOTE-0001: Note one\n\
         history:\n  \"2026-01-01T00:00:00.000000Z\": doing\n\
         priority: 5\n\
         references:\n  NOTE-0001: Note one\n\
         ---\n\n# Do the thing\n\nWork.\n",
    )
    .unwrap();
    std::fs::write(
        inventory.join("IDEA-0003.md"),
        "---\n\
         id: IDEA-0003\n\
         status: draft\n\
         history:\n  \"2026-01-01T00:00:00.000000Z\": draft\n\
         ---\n\n# An idea\n\nThink.\n",
    )
    .unwrap();
    std::fs::canonicalize(root).unwrap()
}

/// A project that loads but does not verify, in exactly two ways: frontmatter
/// is closed, so the undeclared `bogus` key is one problem, and the `blocked_by`
/// entry points at an id no document answers to, which is the other. Without
/// them every assertion about verify problems would hold whether or not they
/// reach the client.
///
/// The dangling entry is also the only servable shape of a *non-live blocker
/// that is not a release* (ADR-0050 decision 3 lets it release the document
/// precisely because `verify` reports it), so the document view has something
/// to distinguish.
fn messy_project(root: &Path) -> PathBuf {
    let inventory = root.join("inventory");
    std::fs::create_dir_all(&inventory).unwrap();
    std::fs::write(root.join("opys.toml"), CONFIG).unwrap();
    std::fs::write(
        inventory.join("NOTE-0003.md"),
        "---\nid: NOTE-0003\nstatus: open\nbogus: 1\n\
         blocked_by:\n  TASK-0009: Vanished\n\
         history:\n  \"2026-01-01T00:00:00.000000Z\": open\n\
         ---\n\n# Messy\n\nText.\n",
    )
    .unwrap();
    std::fs::canonicalize(root).unwrap()
}

/// A project whose config will not parse: discovery keeps it, the actor cannot
/// load it, and `/api/projects` has to say so.
fn broken_project(root: &Path) -> PathBuf {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("opys.toml"), "base = = nope\n").unwrap();
    std::fs::canonicalize(root).unwrap()
}

fn write_allowlist(config: &Path, projects: &[&PathBuf]) {
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    let body: String = projects
        .iter()
        .map(|p| format!("[[project]]\npath = {:?}\n\n", p.display().to_string()))
        .collect();
    std::fs::write(config, body).unwrap();
}

/// A served node over a tempdir.
///
/// Field order is load-bearing, and the tempdir comes last: Rust drops fields in
/// declaration order, so the [`AppState`] — and with it the manager and its
/// corpus actors — goes first, and the actors are told to stop before their
/// inventory is pulled out from under them.
struct Fixture {
    state: AppState,
    cid: String,
    messy_cid: String,
    broken_cid: String,
    root: PathBuf,
    /// The roots behind `messy_cid`/`broken_cid`, so a test can run the real CLI
    /// against the same corpus the node is serving.
    messy: PathBuf,
    broken: PathBuf,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = project(&dir.path().join("proj"));
        let messy = messy_project(&dir.path().join("messy"));
        let broken = broken_project(&dir.path().join("broken"));
        let config = dir.path().join("config/server.toml");
        write_allowlist(&config, &[&root, &messy, &broken]);

        let (events, _) = broadcast::channel(32);
        let mut manager = Manager::new(config, events.clone(), backend);
        manager.rescan().unwrap();
        let cids = manager.cids();
        assert_eq!(cids.len(), 3, "every project is served");
        let cid_of = |want: &PathBuf| {
            cids.iter()
                .find(|cid| manager.get(cid).is_some_and(|h| h.corpus.root == *want))
                .unwrap_or_else(|| panic!("no corpus for {}", want.display()))
                .clone()
        };
        let (cid, messy_cid, broken_cid) = (cid_of(&root), cid_of(&messy), cid_of(&broken));

        Fixture {
            state: AppState::new(Arc::new(Mutex::new(manager)), events),
            cid,
            messy_cid,
            broken_cid,
            root,
            messy,
            broken,
            _dir: dir,
        }
    }

    async fn send(&self, request: Request<Body>) -> (StatusCode, Value) {
        let response = api::router(self.state.clone())
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
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

#[tokio::test]
async fn health_reports_ok_and_version() {
    let fx = Fixture::new();
    let (status, body) = fx.get("/api/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert!(body["started"].is_string(), "{body}");
}

/// The `--no-default-features` build's half of the UI-route contract: the API
/// is untouched, and the two UI routes say *why* they have nothing to serve.
///
/// 501 rather than 404 on purpose. A 404 would send an operator looking for a
/// typo in their URL; the URL is fine, and the answer is that this binary was
/// built without a UI. `tests/assets.rs` covers the feature-on side.
#[cfg(not(feature = "web-ui"))]
#[tokio::test]
async fn the_ui_routes_explain_a_build_without_the_web_ui_feature() {
    let fx = Fixture::new();

    let (status, body) = fx.get("/").await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    let message = body["error"].as_str().unwrap_or_default();
    assert!(message.contains("web-ui"), "{body}");

    let (status, body) = fx.get("/ui/index.js").await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert!(body["error"]
        .as_str()
        .unwrap_or_default()
        .contains("web-ui"));

    // The point of the feature: everything that is not the UI still works.
    let (status, body) = fx.get("/api/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn projects_carry_counts_and_surface_a_broken_config() {
    let fx = Fixture::new();
    let (status, body) = fx.get("/api/projects").await;
    assert_eq!(status, StatusCode::OK);

    let corpora: Vec<&Value> = body
        .as_array()
        .expect("a list of projects")
        .iter()
        .flat_map(|p| p["corpora"].as_array().expect("corpora").iter())
        .collect();
    assert_eq!(corpora.len(), 3, "{body:#}");
    let find = |cid: &str| {
        corpora
            .iter()
            .find(|c| c["cid"] == cid)
            .unwrap_or_else(|| panic!("{cid} is listed"))
    };

    let good = find(&fx.cid);
    assert_eq!(good["root"], fx.root.display().to_string());
    assert_eq!(
        good["base"],
        fx.root.join("inventory").display().to_string()
    );
    assert_eq!(good["is_primary"], true);
    assert_eq!(good["doc_count"], 3);
    assert_eq!(good["verify_problems"], 0, "{good:#}");
    assert!(good["loaded_at"].is_string(), "{good:#}");
    assert!(good.get("error").is_none(), "a healthy corpus has no error");

    // The count is the badge a UI shows, so it has to be the real one.
    let messy = find(&fx.messy_cid);
    assert_eq!(messy["doc_count"], 1, "{messy:#}");
    assert_eq!(messy["verify_problems"], 2, "{messy:#}");

    let broken = find(&fx.broken_cid);
    assert!(
        broken["error"].as_str().is_some_and(|e| !e.is_empty()),
        "{broken:#}"
    );
    assert_eq!(broken["doc_count"], Value::Null, "no counts without a load");
    assert_eq!(broken["verify_problems"], Value::Null);
    assert_eq!(broken["loaded_at"], Value::Null);
}

#[tokio::test]
async fn docs_filters_are_optional_and_and_combined() {
    let fx = Fixture::new();
    let (status, all) = fx.get(&format!("/api/corpus/{}/docs", fx.cid)).await;
    assert_eq!(status, StatusCode::OK);
    let all = all.as_array().unwrap().clone();
    assert_eq!(all.len(), 3, "{all:#?}");
    let note = all.iter().find(|d| d["id"] == "NOTE-0001").unwrap();
    assert_eq!(note["type"], "note");
    assert_eq!(note["status"], "open");
    assert_eq!(note["title"], "Note one");
    assert_eq!(note["tags"], json!(["alpha", "shared"]));
    assert_eq!(note["path"], "inventory/NOTE-0001.md");
    assert_eq!(note["updated"], "2026-01-01T00:00:00Z");
    // The board's relation badges. Blocked is derived (ADR-0050 decision 3), so
    // the summary names the live blockers rather than counting map entries: the
    // fixture's note blocks the task, `note` declares no `releases_blocks` so it
    // still holds, and nothing blocks the note.
    assert_eq!(note["blocked"], false);
    assert_eq!(note["live_blockers"], json!([]));
    assert_eq!(note["blocks"], 1);
    // Priority rides along when the frontmatter has one (the board orders a
    // column by it); a document without one says null, which sorts last.
    assert_eq!(note["priority"], json!(null));
    let task = all.iter().find(|d| d["id"] == "TASK-0002").unwrap();
    assert_eq!(task["blocked"], true);
    assert_eq!(task["live_blockers"], json!(["NOTE-0001"]));
    assert_eq!(task["blocks"], 0);
    assert_eq!(task["priority"], 5);

    let (_, by_type) = fx
        .get(&format!("/api/corpus/{}/docs?type=task", fx.cid))
        .await;
    assert_eq!(by_type.as_array().unwrap().len(), 1);
    assert_eq!(by_type[0]["id"], "TASK-0002");

    // The tag on its own, so it is the only thing that can be discriminating:
    // only the note carries `alpha`, both documents carry `shared`.
    let (_, tagged) = fx
        .get(&format!("/api/corpus/{}/docs?tag=alpha", fx.cid))
        .await;
    assert_eq!(tagged.as_array().unwrap().len(), 1, "{tagged:#}");
    assert_eq!(tagged[0]["id"], "NOTE-0001");
    let (_, shared) = fx
        .get(&format!("/api/corpus/{}/docs?tag=shared", fx.cid))
        .await;
    assert_eq!(shared.as_array().unwrap().len(), 2, "{shared:#}");
    let (_, absent) = fx
        .get(&format!("/api/corpus/{}/docs?tag=nobody", fx.cid))
        .await;
    assert!(absent.as_array().unwrap().is_empty(), "{absent:#}");

    // Every filter at once, all satisfied by the note.
    let (_, narrowed) = fx
        .get(&format!(
            "/api/corpus/{}/docs?type=note&status=open&tag=alpha",
            fx.cid
        ))
        .await;
    assert_eq!(narrowed.as_array().unwrap().len(), 1);
    assert_eq!(narrowed[0]["id"], "NOTE-0001");

    // …and they are AND-combined, so one mismatch is enough.
    let (_, contradictory) = fx
        .get(&format!(
            "/api/corpus/{}/docs?type=note&status=doing",
            fx.cid
        ))
        .await;
    assert!(contradictory.as_array().unwrap().is_empty());

    // An empty value is a cleared filter, not a match on "".
    let (_, empty_filter) = fx
        .get(&format!("/api/corpus/{}/docs?type=&status=", fx.cid))
        .await;
    assert_eq!(
        empty_filter.as_array().unwrap().len(),
        3,
        "{empty_filter:#}"
    );
}

#[tokio::test]
async fn doc_exposes_relations_and_the_rendered_body() {
    let fx = Fixture::new();
    let (status, body) = fx
        .get(&format!("/api/corpus/{}/doc/NOTE-0001", fx.cid))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], "NOTE-0001");
    assert_eq!(body["type"], "note");
    assert_eq!(body["status"], "open");
    assert_eq!(body["title"], "Note one");
    assert_eq!(body["path"], "inventory/NOTE-0001.md");
    assert_eq!(body["tags"], json!(["alpha", "shared"]));
    assert_eq!(body["updated"], "2026-01-01T00:00:00Z");
    assert_eq!(body["references"], json!({"TASK-0002": "Do the thing"}));
    assert_eq!(body["blocks"], json!({"TASK-0002": "Do the thing"}));
    assert_eq!(
        body["blocked_by"],
        json!({}),
        "absent means empty, not null"
    );
    assert_eq!(body["fields"]["status"], "open");
    assert!(body["body"].as_str().unwrap().contains("world."));
    // comrak's `unsafe_` is off and must stay off: bodies are user content.
    let html = body["body_html"].as_str().unwrap();
    assert!(!html.contains("<script>"), "{html}");
    assert!(html.contains("world."), "{html}");
}

/// The status vocabulary travels with the document, so the UI's
/// status picker never has to read `opys.toml`. Terminal statuses are left out
/// because `set-status` refuses them — `close` is the only way there — and
/// `closable` says whether that door exists at all.
///
/// This is the type's whole vocabulary, unchanged by the status graph: narrowing
/// it would have answered a different question under the same name.
/// `allowed_targets` (below) is where the graph shows up.
#[tokio::test]
async fn doc_carries_the_statuses_a_write_would_accept() {
    let fx = Fixture::new();

    let (status, note) = fx
        .get(&format!("/api/corpus/{}/doc/NOTE-0001", fx.cid))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        note["allowed_statuses"],
        json!(["open"]),
        "`closed` is terminal, so set-status cannot reach it: {note:#}"
    );
    assert_eq!(note["closable"], true);

    let (status, task) = fx
        .get(&format!("/api/corpus/{}/doc/TASK-0002", fx.cid))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        task["allowed_statuses"],
        json!(["todo", "doing"]),
        "{task:#}"
    );
    assert_eq!(task["closable"], true);

    // Which sections `opys log` writes to travels the same way (FEAT-0056), so
    // the entry box never has to read `opys.toml` either — and a type with no
    // log section says so by carrying none.
    assert_eq!(task["log_sections"], json!(["Progress"]), "{task:#}");
    assert_eq!(note["log_sections"], json!([]), "{note:#}");
}

/// `closable` is false for two different reasons, and the payload has to say
/// which: a type may declare a whole status graph and still have no terminal
/// status — the shipped `feature` type, and this repo's `feature` and `adr`.
///
/// Without `has_terminal_status` a client can only guess from `allowed_targets`,
/// which is non-null for *any* graph — and would then tell the reader to "move
/// it along first" towards a close that can never happen.
#[tokio::test]
async fn a_graph_without_a_terminal_status_is_never_closable() {
    let fx = Fixture::new();

    let (status, idea) = fx
        .get(&format!("/api/corpus/{}/doc/IDEA-0003", fx.cid))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(idea["status"], "draft");
    assert_eq!(
        idea["allowed_targets"],
        json!(["live"]),
        "`idea` declares a graph, so the menu is its edges: {idea:#}"
    );
    assert_eq!(
        idea["closable"], false,
        "no terminal status, so no close: {idea:#}"
    );
    assert_eq!(
        idea["has_terminal_status"], false,
        "…and the reason is the missing terminal status, not the graph: {idea:#}"
    );

    // The other shape, to pin that the two fields really do differ: `task` has
    // both a graph and a terminal status, and `note` has a terminal status
    // without a graph.
    let (_, task) = fx
        .get(&format!("/api/corpus/{}/doc/TASK-0002", fx.cid))
        .await;
    assert_eq!(task["has_terminal_status"], true, "{task:#}");
    let (_, note) = fx
        .get(&format!("/api/corpus/{}/doc/NOTE-0001", fx.cid))
        .await;
    assert_eq!(note["allowed_targets"], json!(null), "{note:#}");
    assert_eq!(note["has_terminal_status"], true, "{note:#}");
}

/// Where *this* document may go, computed by the node from the type's declared
/// status graph with the same engine helper `set-status`'s guard reads — so a
/// menu built from it cannot offer a move the node would then refuse. `null`,
/// not `[]`, for a type that declares no graph: "everything" and "nothing" are
/// different answers, and the client needs to tell them apart to fall back.
#[tokio::test]
async fn doc_carries_the_moves_its_type_declares() {
    let fx = Fixture::new();

    let (status, note) = fx
        .get(&format!("/api/corpus/{}/doc/NOTE-0001", fx.cid))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        note["allowed_targets"],
        json!(null),
        "`note` declares no transitions, so every status is reachable: {note:#}"
    );

    let (status, task) = fx
        .get(&format!("/api/corpus/{}/doc/TASK-0002", fx.cid))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        task["allowed_targets"],
        json!(["todo"]),
        "TASK-0002 is `doing`, whose declared targets are todo and the terminal \
         done — and a terminal status is `close`'s, not the menu's: {task:#}"
    );
    // The edge into `done` is still real: it is what makes the task closable.
    assert_eq!(task["closable"], true);
}

/// Blocked is derived (ADR-0050 decision 3), and the document view says which
/// `blocked_by` entries still hold. The map alone cannot: liveness depends on
/// the rest of the corpus and on the blocker type's `releases_blocks`, so a
/// client deriving it would be a second interpretation of `opys.toml`.
#[tokio::test]
async fn doc_says_which_of_its_blockers_still_hold() {
    let fx = Fixture::new();

    let (status, task) = fx
        .get(&format!("/api/corpus/{}/doc/TASK-0002", fx.cid))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(task["blocked_by"], json!({"NOTE-0001": "Note one"}));
    // `note` declares no `releases_blocks`, so an open one still holds.
    assert_eq!(task["live_blockers"], json!(["NOTE-0001"]));
    assert_eq!(task["blocked"], true);

    assert_eq!(
        task["dangling_blockers"],
        json!([]),
        "the blocker is a real document: {task:#}"
    );

    let (status, note) = fx
        .get(&format!("/api/corpus/{}/doc/NOTE-0001", fx.cid))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(note["live_blockers"], json!([]));
    assert_eq!(note["blocked"], false);

    // The other way an entry stops holding, and the reason `live_blockers`
    // alone cannot explain itself: nothing answers to the id. The document is
    // released all the same — that is the recorded judgement call — but a view
    // that called this "released" would be naming a status transition for a
    // document that is not there, so the payload separates the two.
    let (status, messy) = fx
        .get(&format!("/api/corpus/{}/doc/NOTE-0003", fx.messy_cid))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(messy["blocked_by"], json!({"TASK-0009": "Vanished"}));
    assert_eq!(messy["live_blockers"], json!([]));
    assert_eq!(messy["dangling_blockers"], json!(["TASK-0009"]));
    assert_eq!(messy["blocked"], false);
}

#[tokio::test]
async fn an_unknown_document_is_404() {
    let fx = Fixture::new();
    let (status, body) = fx
        .get(&format!("/api/corpus/{}/doc/NOTE-9999", fx.cid))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("NOTE-9999")),
        "{body}"
    );
}

#[tokio::test]
async fn query_answers_select_and_refuses_anything_else() {
    let fx = Fixture::new();
    let uri = format!("/api/corpus/{}/query", fx.cid);

    let (status, body) = fx
        .post(
            &uri,
            json!({"sql": "SELECT id, status FROM docs WHERE type = $1", "params": ["note"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["columns"], json!(["id", "status"]));
    assert_eq!(body["rows"], json!([["NOTE-0001", "open"]]));

    // The engine's plan guard is the single place this is decided; the API just
    // reports what it said.
    let (status, body) = fx.post(&uri, json!({"sql": "DELETE FROM docs"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"].as_str().is_some_and(|e| e.contains("SELECT")),
        "{body}"
    );

    // …and nothing was deleted. Read through the store, not the summary cache:
    // the cache is rebuilt only by a load, so it could not notice either way.
    let (status, body) = fx.post(&uri, json!({"sql": "SELECT id FROM docs"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"].as_array().unwrap().len(), 3, "{body}");

    let (status, body) = fx.post(&uri, json!({"sql": "SELECT * FROM nope"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].is_string(), "{body}");
}

/// Malformed input must still come back in the one error shape, with one of the
/// documented statuses — which means not letting axum's own rejections render
/// their plain-text bodies or their 415/422.
#[tokio::test]
async fn every_rejection_is_a_json_error() {
    let fx = Fixture::new();
    let query = format!("/api/corpus/{}/query", fx.cid);

    let syntax = Request::builder()
        .method("POST")
        .uri(&query)
        .header("content-type", "application/json")
        .body(Body::from("{not json"))
        .unwrap();
    // No `sql` key: axum would answer 422.
    let missing_field = Request::builder()
        .method("POST")
        .uri(&query)
        .header("content-type", "application/json")
        .body(Body::from(r#"{"params": []}"#))
        .unwrap();
    // What `fetch(url, {method: 'POST', body: JSON.stringify(x)})` sends by
    // default: axum would answer 415.
    let no_content_type = Request::builder()
        .method("POST")
        .uri(&query)
        .body(Body::from(r#"{"sql": "SELECT 1"}"#))
        .unwrap();
    // A multi-select filter serialized as repeated keys.
    let repeated_filter = Request::builder()
        .uri(format!("/api/corpus/{}/docs?type=note&type=task", fx.cid))
        .body(Body::empty())
        .unwrap();
    // `/api/events` opened in an address bar rather than upgraded.
    let plain_events = Request::builder()
        .uri("/api/events")
        .body(Body::empty())
        .unwrap();

    for request in [
        syntax,
        missing_field,
        no_content_type,
        repeated_filter,
        plain_events,
    ] {
        let uri = request.uri().to_string();
        let (status, body) = fx.send(request).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {body}");
        assert!(body["error"].is_string(), "{uri}: {body}");
    }
}

/// Routing-level failures answer in the same shape: axum's own would be an empty
/// body a client's `res.json()` cannot parse.
#[tokio::test]
async fn unknown_routes_and_methods_are_json_errors() {
    let fx = Fixture::new();
    let (status, body) = fx.get("/api/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("/api/nope")),
        "{body}"
    );

    let (status, body) = fx.post("/api/health", json!({})).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert!(body["error"].is_string(), "{body}");
}

#[tokio::test]
async fn verify_reports_problems_and_when_they_were_computed() {
    let fx = Fixture::new();
    let (status, body) = fx.get(&format!("/api/corpus/{}/verify", fx.cid)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["problems"], json!([]), "{body:#}");
    assert!(body["loaded_at"].is_string(), "{body}");
    assert_eq!(body["ok"], true);

    // A corpus that loads but does not verify: the problems themselves have to
    // reach the client, not just a count.
    let (status, body) = fx
        .get(&format!("/api/corpus/{}/verify", fx.messy_cid))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], false, "{body:#}");
    let problems = body["problems"].as_array().expect("a list of problems");
    assert_eq!(problems.len(), 2, "{body:#}");
    assert!(
        problems
            .iter()
            .all(|p| p.as_str().is_some_and(|p| p.contains("NOTE-0003"))),
        "every problem names the document: {body:#}"
    );
    assert!(body["loaded_at"].is_string(), "{body}");

    // The broken corpus never loaded: no problems to report, but a reason.
    let (status, body) = fx
        .get(&format!("/api/corpus/{}/verify", fx.broken_cid))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], false);
    assert!(body["load_error"].is_string(), "{body}");
}

/// A corpus that never loaded has no answers, and must not pretend otherwise:
/// an empty list reads as "the inventory is empty" and a 404 as "that document
/// was deleted", when the truth is that the project would not parse.
#[tokio::test]
async fn reads_on_a_corpus_that_never_loaded_are_server_errors() {
    let fx = Fixture::new();
    let (status, body) = fx.get(&format!("/api/corpus/{}/docs", fx.broken_cid)).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("not loaded")),
        "{body}"
    );

    let (status, body) = fx
        .get(&format!("/api/corpus/{}/doc/NOTE-0001", fx.broken_cid))
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");

    // A valid SELECT against a corpus that will not load is our problem, not the
    // caller's: 400 would tell them to fix SQL that is already correct.
    let (status, body) = fx
        .post(
            &format!("/api/corpus/{}/query", fx.broken_cid),
            json!({"sql": "SELECT id FROM docs"}),
        )
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("not loaded")),
        "{body}"
    );
}

#[tokio::test]
async fn an_unknown_corpus_is_404_on_every_route() {
    let fx = Fixture::new();
    for uri in [
        "/api/corpus/nope/docs",
        "/api/corpus/nope/doc/NOTE-0001",
        "/api/corpus/nope/verify",
    ] {
        let (status, body) = fx.get(uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert!(
            body["error"].as_str().is_some_and(|e| e.contains("nope")),
            "{uri}: {body}"
        );
    }
    for (uri, body) in [
        ("/api/corpus/nope/query", json!({"sql": "SELECT 1"})),
        // The write route resolves the cid itself rather than through
        // `with_corpus`, so it is the one 404 here that is not shared code.
        (
            "/api/corpus/nope/action",
            json!({"action": "tag", "id": "NOTE-0001", "add": "x"}),
        ),
    ] {
        let (status, answer) = fx.post(uri, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {answer}");
        assert!(
            answer["error"].as_str().is_some_and(|e| e.contains("nope")),
            "{uri}: {answer}"
        );
    }
}

/// The bind address keeps other machines out; it does nothing about the user's
/// own browser. A page they visit can `fetch` a loopback URL, and a WebSocket
/// handshake is not subject to the same-origin policy at all — so the node has
/// to refuse for itself.
#[tokio::test]
async fn a_foreign_origin_is_refused_everywhere() {
    let fx = Fixture::new();
    for uri in [
        "/api/health",
        "/api/projects",
        &format!("/api/corpus/{}/docs", fx.cid),
        "/api/events",
    ] {
        let request = Request::builder()
            .uri(uri)
            .header("origin", "https://evil.example")
            .header("host", "127.0.0.1:6797")
            .body(Body::empty())
            .unwrap();
        let (status, body) = fx.send(request).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {body}");
        assert!(
            body["error"]
                .as_str()
                .is_some_and(|e| e.contains("evil.example")),
            "{uri}: {body}"
        );
    }
}

/// DNS rebinding: the attacker's name resolves to 127.0.0.1, so the request
/// arrives — carrying their host, which is what gives it away.
#[tokio::test]
async fn a_rebound_host_is_refused_and_a_local_one_is_served() {
    let fx = Fixture::new();
    let rebound = Request::builder()
        .uri("/api/projects")
        .header("host", "evil.attacker.test")
        .body(Body::empty())
        .unwrap();
    let (status, body) = fx.send(rebound).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("evil.attacker.test")),
        "{body}"
    );

    // The real thing: a browser on this machine, and a dev server on another
    // loopback port, both of which have to keep working.
    for (host, origin) in [
        ("127.0.0.1:6797", None),
        ("localhost:6797", Some("http://localhost:6797")),
        ("[::1]:6797", Some("http://[::1]:6797")),
        ("127.0.0.1:6797", Some("http://localhost:5173")),
    ] {
        let mut request = Request::builder().uri("/api/health").header("host", host);
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        let (status, body) = fx.send(request.body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::OK, "{host} / {origin:?}: {body}");
    }
}

/// Run the real `opys verify` in-process against `root` and return its exit
/// code, or the error the CLI would have exited 2 on.
///
/// `opys_engine::run` is the same dispatch the binary calls, so this is the CLI's
/// own verdict rather than a transcription of it.
fn cli_verify(root: &Path) -> opys_engine::error::Result<i32> {
    let argv = [
        "opys".to_string(),
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
        "verify".to_string(),
    ];
    opys_engine::run(Cli::parse_from(argv), Box::new(MarkdownLocal))
}

/// FEAT-0029's test plan claims the node's verify status "matches `opys verify`
/// exit semantics". This pins that — including the one case where it does not
/// hold, so the divergence is recorded rather than assumed away.
///
/// The problem *lists* are identical by construction (both sides call
/// `verify::collect_problems`), so asserting on those would prove nothing. What
/// is worth pinning is the verdict a caller acts on: the CLI's exit code against
/// the API's `ok`.
#[tokio::test]
async fn verify_agrees_with_the_cli_exit_code() {
    let fx = Fixture::new();

    // Clean corpus: exit 0, ok.
    assert_eq!(cli_verify(&fx.root).ok(), Some(0), "a clean corpus exits 0");
    let (_, body) = fx.get(&format!("/api/corpus/{}/verify", fx.cid)).await;
    assert_eq!(body["ok"], true, "{body:#}");
    assert_eq!(
        body["problems"].as_array().map(Vec::len),
        Some(0),
        "{body:#}"
    );

    // Content problems: exit 1, not ok, and the problems are listed.
    assert_eq!(
        cli_verify(&fx.messy).ok(),
        Some(1),
        "content problems are the CI-gate exit code, not an error"
    );
    let (_, body) = fx
        .get(&format!("/api/corpus/{}/verify", fx.messy_cid))
        .await;
    assert_eq!(body["ok"], false, "{body:#}");
    assert!(
        !body["problems"].as_array().unwrap().is_empty(),
        "the node has to say what is wrong, not just that something is: {body:#}"
    );
    assert_eq!(
        body["load_error"],
        Value::Null,
        "a content problem is not a load failure: {body:#}"
    );

    // The divergence. A config that will not parse is an `OpysError` for the
    // CLI — exit 2, a different class from "this corpus has problems" — while
    // the API answers `ok: false` for both. `ok` alone cannot tell them apart;
    // `load_error` is what distinguishes them, and an empty problem list is the
    // tell that nothing was ever checked.
    assert!(
        cli_verify(&fx.broken).is_err(),
        "a config that will not parse is an error, not a problem list"
    );
    let (_, body) = fx
        .get(&format!("/api/corpus/{}/verify", fx.broken_cid))
        .await;
    assert_eq!(body["ok"], false, "{body:#}");
    assert!(
        body["load_error"].as_str().is_some_and(|e| !e.is_empty()),
        "the reason has to reach the client: {body:#}"
    );
    assert_eq!(
        body["problems"].as_array().map(Vec::len),
        Some(0),
        "nothing was verified, so nothing may be reported as verified: {body:#}"
    );
}

// ---------------------------------------------------------------------------
// The pick oracle endpoint: `GET /api/projects/{key}/next` (FEAT-0057)
// ---------------------------------------------------------------------------

/// The group key `/api/projects` publishes for the corpus this fixture serves
/// first — the same string `/api/group/{key}/union` takes, and the one the pick
/// endpoint is addressed by.
async fn project_key(fx: &Fixture) -> String {
    let (status, body) = fx.get("/api/projects").await;
    assert_eq!(status, StatusCode::OK);
    body.as_array()
        .unwrap()
        .iter()
        .find(|p| {
            p["corpora"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["cid"] == fx.cid.as_str())
        })
        .unwrap_or_else(|| panic!("no group holds {}: {body:#}", fx.cid))["key"]
        .as_str()
        .unwrap()
        .to_string()
}

/// What `opys_engine::next::pick` answers over the same corpus, as JSON.
///
/// Loaded through the real backend from the same root the node is serving, so
/// this is the engine's own verdict rather than a transcription of it.
fn engine_pick(root: &Path, q: &opys_engine::next::PickQuery) -> Value {
    let prj = opys_engine::Project::open(&root.to_string_lossy()).unwrap();
    let (mut store, _) = MarkdownLocal.load(&prj).unwrap();
    let docs: Vec<opys_engine::doc::Doc> = store
        .all_docs()
        .unwrap()
        .into_iter()
        .map(|(_, d)| d)
        .collect();
    let out = opys_engine::next::pick(&prj, &docs, q, opys_engine::observe::now());
    serde_json::to_value(out).unwrap()
}

/// The endpoint and the engine function answer the same bytes, because they are
/// the same function and the same serde types: `opys/tests/cli.rs` pins the
/// CLI's rendering of that payload, and this pins the node's, so neither
/// surface can drift on its own.
///
/// It also pins what the payload *says* about this corpus: NOTE-0001 is
/// pickable at `open`, and TASK-0002 is not pickable at `doing` because
/// NOTE-0001 still blocks it — the derived-blocked rule, evaluated on the
/// corpus the node serves (ADR-0053).
#[tokio::test]
async fn the_next_endpoint_answers_exactly_what_the_engine_does() {
    let fx = Fixture::new();
    let key = project_key(&fx).await;

    let (status, body) = fx
        .get(&format!("/api/projects/{key}/next?status=open"))
        .await;
    assert_eq!(status, StatusCode::OK, "{body:#}");
    assert_eq!(body["schema"], "opys.next/1", "{body:#}");
    assert_eq!(body["items"][0]["id"], "NOTE-0001", "{body:#}");
    assert_eq!(
        body["items"][0]["path"], "inventory/NOTE-0001.md",
        "{body:#}"
    );
    assert_eq!(body["limit"], 10, "the default limit: {body:#}");
    assert_eq!(
        body,
        engine_pick(
            &fx.root,
            &opys_engine::next::PickQuery {
                status: "open".to_string(),
                type_filter: None,
                limit: 10,
            },
        ),
        "the endpoint must serialize the engine's answer verbatim"
    );

    // Blocked by a live NOTE-0001, so nothing is pickable at `doing`.
    let (status, body) = fx
        .get(&format!("/api/projects/{key}/next?status=doing"))
        .await;
    assert_eq!(status, StatusCode::OK, "{body:#}");
    assert_eq!(body["total"], 0, "{body:#}");
    assert_eq!(body["items"].as_array().map(Vec::len), Some(0), "{body:#}");
}

/// `?type=` and `?n=` are the endpoint's spelling of `--type` and `-n`.
#[tokio::test]
async fn the_next_endpoint_takes_the_type_and_limit_filters() {
    let fx = Fixture::new();
    let key = project_key(&fx).await;

    // `todo` is a status only `task` declares, and no task is at it.
    let (status, body) = fx
        .get(&format!("/api/projects/{key}/next?status=todo&type=task"))
        .await;
    assert_eq!(status, StatusCode::OK, "{body:#}");
    assert_eq!(body["type"], "task", "{body:#}");
    assert_eq!(body["items"].as_array().map(Vec::len), Some(0), "{body:#}");
    // An empty `?type=` is a cleared dropdown, not the empty type name.
    let (status, body) = fx
        .get(&format!("/api/projects/{key}/next?status=open&type="))
        .await;
    assert_eq!(status, StatusCode::OK, "{body:#}");
    assert_eq!(body["type"], Value::Null, "{body:#}");
    assert_eq!(body["items"][0]["id"], "NOTE-0001", "{body:#}");

    let (status, body) = fx
        .get(&format!("/api/projects/{key}/next?status=open&n=0"))
        .await;
    assert_eq!(status, StatusCode::OK, "{body:#}");
    assert_eq!(body["limit"], 0, "{body:#}");
    assert_eq!(body["total"], 1, "the limit truncates, it does not filter");
    assert_eq!(body["items"].as_array().map(Vec::len), Some(0), "{body:#}");
}

/// A caller's mistake is a 400, never the 500 an engine error would become: a
/// bad `status` comes back through the actor's inner `Result`, the way user SQL
/// does. A project nobody serves is a 404.
#[tokio::test]
async fn the_next_endpoint_classes_bad_input_as_bad_input() {
    let fx = Fixture::new();
    let key = project_key(&fx).await;

    let (status, body) = fx.get(&format!("/api/projects/{key}/next")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:#}");

    let (status, body) = fx.get(&format!("/api/projects/{key}/next?status=")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:#}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("status"),
        "{body:#}"
    );

    let (status, body) = fx
        .get(&format!("/api/projects/{key}/next?status=nonesuch"))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:#}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("unknown status"),
        "{body:#}"
    );

    let (status, body) = fx
        .get(&format!("/api/projects/{key}/next?status=open&type=nope"))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:#}");

    let (status, body) = fx.get("/api/projects/nobody/next?status=open").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body:#}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("no such project"),
        "{body:#}"
    );
}

// ---------------------------------------------------------------------------
// The flow graph endpoint: `GET /api/projects/{key}/flow` (FEAT-0060)
// ---------------------------------------------------------------------------

/// The node payload for one status, or a panic naming what was there instead.
fn flow_node<'a>(body: &'a Value, status: &str) -> &'a Value {
    body["nodes"]
        .as_array()
        .unwrap_or_else(|| panic!("no nodes: {body:#}"))
        .iter()
        .find(|n| n["status"] == status)
        .unwrap_or_else(|| panic!("no {status} node: {body:#}"))
}

/// The graph is the config's, not the corpus's: every declared status is a
/// node, the edges are the union of the `transitions` maps, and the type that
/// declares no map contributes its statuses and no edges at all.
#[tokio::test]
async fn the_flow_endpoint_draws_the_graph_the_config_declares() {
    let fx = Fixture::new();
    let key = project_key(&fx).await;

    let (status, body) = fx.get(&format!("/api/projects/{key}/flow")).await;
    assert_eq!(status, StatusCode::OK, "{body:#}");
    assert_eq!(body["cid"], fx.cid.as_str(), "{body:#}");
    assert_eq!(body["limit"], 5, "the default per-node cap: {body:#}");

    // Declaration order, which is the order everything ranked in this tool uses
    // — never the alphabetical one a `BTreeMap` walk would give (idea, note, task).
    let types: Vec<&str> = body["types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(types, vec!["note", "task", "idea"], "{body:#}");

    let statuses: Vec<&str> = body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["status"].as_str().unwrap())
        .collect();
    assert_eq!(
        statuses,
        vec!["open", "closed", "todo", "doing", "done", "draft", "live"],
        "{body:#}"
    );

    let edges: Vec<(&str, &str)> = body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["from"].as_str().unwrap(), e["to"].as_str().unwrap()))
        .collect();
    assert_eq!(
        edges,
        vec![
            ("todo", "doing"),
            ("doing", "todo"),
            ("doing", "done"),
            ("draft", "live"),
            ("live", "draft"),
        ],
        "`note` declares no transitions, so it contributes no edges: {body:#}"
    );

    assert!(flow_node(&body, "open")["initial"].as_bool().unwrap());
    assert!(flow_node(&body, "todo")["initial"].as_bool().unwrap());
    assert!(flow_node(&body, "draft")["initial"].as_bool().unwrap());
    assert!(!flow_node(&body, "doing")["initial"].as_bool().unwrap());
    // A terminal node is always empty — `close` deletes the file — so it has to
    // be marked, or it reads as a stage nobody ever reached.
    assert!(flow_node(&body, "done")["terminal"].as_bool().unwrap());
    assert!(flow_node(&body, "closed")["terminal"].as_bool().unwrap());
    assert!(!flow_node(&body, "live")["terminal"].as_bool().unwrap());
    assert!(flow_node(&body, "open")["declared"].as_bool().unwrap());
}

/// The composition guarantee, over the wire: a node lists exactly what the
/// `next` endpoint lists for the same status, item for item.
///
/// Both halves come from the same `next::pick`, and this is what says so at the
/// API boundary — that the two endpoints agree on every node of a real served
/// project, including the empty and the blocked ones. The *ordering* half of
/// the guarantee is not visible here: this fixture's types have disjoint status
/// sets and one document each, so no node holds two items. The merged node with
/// several documents of two types is pinned by
/// `flow::tests::a_merged_node_lists_exactly_what_next_lists_in_the_same_order`.
#[tokio::test]
async fn a_flow_node_lists_exactly_what_next_lists() {
    let fx = Fixture::new();
    let key = project_key(&fx).await;

    let (status, flow) = fx.get(&format!("/api/projects/{key}/flow")).await;
    assert_eq!(status, StatusCode::OK, "{flow:#}");
    let limit = flow["limit"].as_u64().unwrap();

    for node in flow["nodes"].as_array().unwrap() {
        let status_name = node["status"].as_str().unwrap();
        let (code, picked) = fx
            .get(&format!(
                "/api/projects/{key}/next?status={status_name}&n={limit}"
            ))
            .await;
        assert_eq!(code, StatusCode::OK, "{picked:#}");
        assert_eq!(
            node["items"], picked["items"],
            "the {status_name} node and `next` disagree: {node:#} vs {picked:#}"
        );
        assert_eq!(node["pickable"], picked["total"], "{node:#}");
        assert_eq!(node["orphaned"], picked["orphaned"], "{node:#}");
    }
}

/// A blocked document is counted and named, never listed: it is not pickable,
/// and a node offering it would be offering work nobody can take. TASK-0002 is
/// `doing` with a live NOTE-0001 blocking it.
#[tokio::test]
async fn a_blocked_document_is_counted_not_listed() {
    let fx = Fixture::new();
    let key = project_key(&fx).await;

    let (status, body) = fx.get(&format!("/api/projects/{key}/flow")).await;
    assert_eq!(status, StatusCode::OK, "{body:#}");

    let doing = flow_node(&body, "doing");
    assert_eq!(doing["total"], 1, "{doing:#}");
    assert_eq!(doing["pickable"], 0, "{doing:#}");
    assert_eq!(
        doing["items"].as_array().map(Vec::len),
        Some(0),
        "{doing:#}"
    );
    assert_eq!(doing["blocked"], json!(["TASK-0002"]), "{doing:#}");

    // And the identity that keeps the three buckets honest, on every node.
    for node in body["nodes"].as_array().unwrap() {
        let parts = node["pickable"].as_u64().unwrap()
            + node["blocked"].as_array().unwrap().len() as u64
            + node["orphaned"].as_array().unwrap().len() as u64;
        assert_eq!(node["total"].as_u64().unwrap(), parts, "{node:#}");
    }
}

/// `?n=` is the per-node cap, and it truncates the list without touching the
/// count behind it — which is what lets a node say "and N more".
#[tokio::test]
async fn the_flow_endpoint_takes_a_per_node_limit() {
    let fx = Fixture::new();
    let key = project_key(&fx).await;

    let (status, body) = fx.get(&format!("/api/projects/{key}/flow?n=0")).await;
    assert_eq!(status, StatusCode::OK, "{body:#}");
    assert_eq!(body["limit"], 0, "{body:#}");
    let open = flow_node(&body, "open");
    assert_eq!(
        open["pickable"], 1,
        "the limit truncates, it does not filter"
    );
    assert_eq!(open["items"].as_array().map(Vec::len), Some(0), "{open:#}");

    let (status, body) = fx.get(&format!("/api/projects/{key}/flow?n=nope")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:#}");
}

/// A project nobody serves is a 404, worded exactly as the pick endpoint words
/// it — the two share one resolver, and this is what pins that they still do.
#[tokio::test]
async fn the_flow_endpoint_404s_for_a_project_nobody_serves() {
    let fx = Fixture::new();
    let (status, body) = fx.get("/api/projects/nobody/flow").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body:#}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("no such project"),
        "{body:#}"
    );
}

/// The editor's block previews: each block rendered on its own, with the same
/// options as a document body — tables and task lists on, raw HTML escaped.
#[tokio::test]
async fn render_turns_blocks_into_the_bodys_html() {
    let fx = Fixture::new();
    let (status, body) = fx
        .post(
            "/api/render",
            json!({ "markdown": ["- [x] done", "| a |\n|---|\n| 1 |", "<script>x</script>"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let html: Vec<&str> = body["html"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h.as_str().unwrap())
        .collect();
    assert_eq!(html.len(), 3);
    assert!(
        html[0].contains("type=\"checkbox\"") && html[0].contains("checked"),
        "{}",
        html[0]
    );
    assert!(html[1].contains("<table>"), "{}", html[1]);
    assert!(
        !html[2].contains("<script>"),
        "raw HTML must stay escaped: {}",
        html[2]
    );

    let (status, _) = fx
        .post("/api/render", json!({ "markdown": "not a list" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
