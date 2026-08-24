//! The read API over real corpora, driven through the router rather than a
//! socket (TASK-0071).

use std::path::{Path, PathBuf};
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

[types.task]
prefix = "TASK"
statuses = ["todo", "doing", "done"]
default_status = "todo"
terminal_statuses = ["done"]
tags_required = false
"#;

fn backend() -> Box<dyn Backend + Send> {
    Box::new(MarkdownLocal)
}

/// A project with two linked documents of two types, so the filters and the
/// relation maps both have something to say.
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
         references:\n  TASK-0002: Do the thing\n\
         blocks:\n  TASK-0002: Do the thing\n\
         ---\n\n# Note one\n\nHello <script>alert(1)</script> world.\n",
    )
    .unwrap();
    std::fs::write(
        inventory.join("TASK-0002.md"),
        "---\n\
         id: TASK-0002\n\
         status: doing\n\
         tags: [shared]\n\
         references:\n  NOTE-0001: Note one\n\
         blocked_by:\n  NOTE-0001: Note one\n\
         ---\n\n# Do the thing\n\nWork.\n",
    )
    .unwrap();
    std::fs::canonicalize(root).unwrap()
}

/// A project that loads but does not verify: frontmatter is closed, so the
/// undeclared `bogus` key is exactly one problem. Without it every assertion
/// about verify problems would hold whether or not they reach the client.
fn messy_project(root: &Path) -> PathBuf {
    let inventory = root.join("inventory");
    std::fs::create_dir_all(&inventory).unwrap();
    std::fs::write(root.join("opys.toml"), CONFIG).unwrap();
    std::fs::write(
        inventory.join("NOTE-0003.md"),
        "---\nid: NOTE-0003\nstatus: open\nbogus: 1\n---\n\n# Messy\n\nText.\n",
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
    assert_eq!(good["doc_count"], 2);
    assert_eq!(good["verify_problems"], 0, "{good:#}");
    assert!(good["loaded_at"].is_string(), "{good:#}");
    assert!(good.get("error").is_none(), "a healthy corpus has no error");

    // The count is the badge a UI shows, so it has to be the real one.
    let messy = find(&fx.messy_cid);
    assert_eq!(messy["doc_count"], 1, "{messy:#}");
    assert_eq!(messy["verify_problems"], 1, "{messy:#}");

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
    assert_eq!(all.len(), 2, "{all:#?}");
    let note = all.iter().find(|d| d["id"] == "NOTE-0001").unwrap();
    assert_eq!(note["type"], "note");
    assert_eq!(note["status"], "open");
    assert_eq!(note["title"], "Note one");
    assert_eq!(note["tags"], json!(["alpha", "shared"]));
    assert_eq!(note["path"], "inventory/NOTE-0001.md");
    assert_eq!(note["updated"], "2026-01-01T00:00:00Z");

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
        2,
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

/// The status vocabulary travels with the document (TASK-0074), so the UI's
/// status picker never has to read `opys.toml`. Terminal statuses are left out
/// because `set-status` refuses them — `close` is the only way there — and
/// `closable` says whether that door exists at all.
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
    assert_eq!(body["rows"].as_array().unwrap().len(), 2, "{body}");

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
    assert_eq!(problems.len(), 1, "{body:#}");
    assert!(
        problems[0]
            .as_str()
            .is_some_and(|p| p.contains("NOTE-0003")),
        "the problem names the document: {body:#}"
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
