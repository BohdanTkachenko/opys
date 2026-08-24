//! The corpus actor over a real inventory on disk (TASK-0070).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use opys_backend_markdown_local::MarkdownLocal;
use opys_engine::backend::Backend;
use opys_engine::project::Project;
use opys_server::actor::{CorpusHandle, DocFilter, Event};
use opys_server::discover::{self, Corpus};
use tokio::sync::broadcast;

const CONFIG: &str = r#"
base = "inventory"

[types.note]
prefix = "NOTE"
statuses = ["open", "closed"]
default_status = "open"
terminal_statuses = ["closed"]
tags_required = false
"#;

fn write_note(inventory: &Path, n: u32, body: &str) {
    let text = format!("---\nid: NOTE-{n:04}\nstatus: open\n---\n\n# Note {n}\n\n{body}\n");
    std::fs::write(inventory.join(format!("NOTE-{n:04}.md")), text).unwrap();
}

/// A project with one note, returned as the [`Corpus`] discovery would produce.
fn fixture(root: &Path) -> Corpus {
    let inventory = root.join("inventory");
    std::fs::create_dir_all(&inventory).unwrap();
    std::fs::write(root.join("opys.toml"), CONFIG).unwrap();
    write_note(&inventory, 1, "Hello <script>alert(1)</script> world.");

    let groups = discover::group(std::slice::from_ref(&root.to_path_buf()));
    groups
        .into_iter()
        .next()
        .expect("one group")
        .corpora
        .into_iter()
        .next()
        .expect("one corpus")
}

/// Wait for the next broadcast event, without needing a tokio runtime.
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

fn spawn(corpus: Corpus) -> (CorpusHandle, broadcast::Receiver<Event>) {
    let (tx, rx) = broadcast::channel(32);
    let handle = CorpusHandle::spawn(corpus, Box::new(MarkdownLocal), tx);
    (handle, rx)
}

/// **The deadlock regression test.** A warm store that keeps the inventory lock
/// makes every CLI invocation against this project wait out
/// `OPYS_LOCK_TIMEOUT_MS` and then fail. The actor must release it the instant
/// the load returns.
#[test]
fn reload_releases_the_inventory_lock() {
    // Shorten the wait so a regression fails fast instead of hanging for 10 s.
    std::env::set_var("OPYS_LOCK_TIMEOUT_MS", "2000");
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let corpus = fixture(&root);

    let (handle, _rx) = spawn(corpus);
    handle.reload().expect("the actor loads");

    // Exactly what a concurrent `opys` invocation does.
    let prj = Project::open(&root.to_string_lossy()).unwrap();
    let started = Instant::now();
    let loaded = MarkdownLocal.load(&prj);
    let elapsed = started.elapsed();

    let (mut store, _errors) = loaded.expect("a CLI-style load must not be blocked by the server");
    assert!(
        elapsed < Duration::from_millis(1500),
        "took {elapsed:?} — the warm store is holding the inventory lock"
    );
    drop(store.take_lock());
    handle.shutdown();
}

#[test]
fn docs_and_doc_and_query_answer_from_the_warm_store() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let corpus = fixture(&root);
    let (handle, _rx) = spawn(corpus);

    let all = handle.docs(DocFilter::default()).unwrap();
    assert_eq!(all.len(), 1, "{all:#?}");
    assert_eq!(all[0].id, "NOTE-0001");
    assert_eq!(all[0].type_name, "note");
    assert_eq!(all[0].status, "open");
    assert_eq!(all[0].title, "Note 1");
    assert_eq!(all[0].path, "inventory/NOTE-0001.md");

    // Filters are equality over the cached summaries.
    let closed = handle
        .docs(DocFilter {
            status: Some("closed".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(closed.is_empty());

    let doc = handle.doc("NOTE-0001").unwrap().expect("the note");
    assert_eq!(doc.id, "NOTE-0001");
    assert!(doc.body.contains("world."), "{}", doc.body);
    assert_eq!(
        doc.fields.get("status").and_then(|v| v.as_str()),
        Some("open")
    );
    // Raw HTML in a body stays escaped: comrak's `unsafe_` is off and must be.
    assert!(
        !doc.body_html.contains("<script>"),
        "raw HTML must not survive rendering: {}",
        doc.body_html
    );
    assert!(doc.body_html.contains("world."), "{}", doc.body_html);

    assert!(handle.doc("NOTE-9999").unwrap().is_none());

    let result = handle
        .query("SELECT id, status FROM docs", &[])
        .unwrap()
        .expect("a valid query");
    assert_eq!(result.columns, vec!["id", "status"]);
    assert_eq!(result.rows, vec![vec!["NOTE-0001", "open"]]);

    // A bad query is the user's problem, not the actor's.
    let bad = handle.query("SELECT * FROM nope", &[]).unwrap();
    assert!(bad.is_err(), "{bad:?}");

    handle.shutdown();
}

#[test]
fn external_edit_triggers_debounced_reload() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let corpus = fixture(&root);
    let (handle, mut rx) = spawn(corpus);

    // The load the actor does on startup.
    let first = next_event(&mut rx, Duration::from_secs(2));
    assert!(
        matches!(first, Some(Event::CorpusReloaded { docs: 1, .. })),
        "expected the initial load, got {first:?}"
    );

    // An edit nobody told the server about.
    write_note(&root.join("inventory"), 2, "Second.");

    let event = next_event(&mut rx, Duration::from_secs(2));
    match event {
        Some(Event::CorpusReloaded { docs, .. }) => assert_eq!(docs, 2, "the cache must catch up"),
        other => panic!("expected a reload after an external edit, got {other:?}"),
    }
    assert_eq!(handle.docs(DocFilter::default()).unwrap().len(), 2);

    // One write is one reload: the debounce collapses the create/write/close
    // burst into a single pass.
    let extra = next_event(&mut rx, Duration::from_millis(800));
    assert!(extra.is_none(), "expected no second reload, got {extra:?}");

    handle.shutdown();
}

#[test]
fn verify_problems_are_cached_and_refreshed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let corpus = fixture(&root);
    let inventory = root.join("inventory");
    let (handle, mut rx) = spawn(corpus);

    let clean = handle.verify().unwrap();
    assert!(clean.ok, "a fresh fixture should be clean: {clean:#?}");
    assert!(clean.problems.is_empty());
    assert!(clean.loaded_at.is_some());
    assert!(clean.load_error.is_none());
    let _ = next_event(&mut rx, Duration::from_secs(2));

    // Frontmatter is closed, so an undeclared key is a verify problem.
    std::fs::write(
        inventory.join("NOTE-0002.md"),
        "---\nid: NOTE-0002\nstatus: open\nbogus: 1\n---\n\n# Broken\n\nText.\n",
    )
    .unwrap();
    assert!(
        next_event(&mut rx, Duration::from_secs(2)).is_some(),
        "the watcher should have noticed the new file"
    );

    let dirty = handle.verify().unwrap();
    assert!(!dirty.ok, "the problem should surface: {dirty:#?}");
    assert!(
        dirty.problems.iter().any(|p| p.contains("NOTE-0002")),
        "{:#?}",
        dirty.problems
    );

    // …and clears again when the cause goes away.
    std::fs::remove_file(inventory.join("NOTE-0002.md")).unwrap();
    assert!(next_event(&mut rx, Duration::from_secs(2)).is_some());
    let healed = handle.verify().unwrap();
    assert!(
        healed.ok,
        "problems must be recomputed, not accumulated: {healed:#?}"
    );

    handle.shutdown();
}

/// A load *reads* every document, and inotify reports reads. If those come back
/// as change events the corpus reloads itself forever, pinning a core for as
/// long as the server runs. A quiet corpus must stay quiet.
#[test]
fn a_quiet_corpus_does_not_reload_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let corpus = fixture(&root);
    let (handle, mut rx) = spawn(corpus);

    let first = next_event(&mut rx, Duration::from_secs(2));
    assert!(first.is_some(), "the initial load should broadcast");

    let extra = next_event(&mut rx, Duration::from_secs(2));
    assert!(
        extra.is_none(),
        "nothing touched the corpus, so nothing should reload it: {extra:?}"
    );
    handle.shutdown();
}

/// A corpus that will not load leaves the previous answers in place and says
/// why, rather than taking the project out of the server.
#[test]
fn a_broken_config_leaves_reads_working_and_reports_the_error() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let corpus = fixture(&root);
    let (handle, _rx) = spawn(corpus);
    assert_eq!(handle.docs(DocFilter::default()).unwrap().len(), 1);

    std::fs::write(root.join("opys.toml"), "this is = = not toml").unwrap();
    handle.reload().unwrap();

    let status = handle.verify().unwrap();
    assert!(!status.ok);
    assert!(status.load_error.is_some(), "{status:#?}");
    assert_eq!(
        handle.docs(DocFilter::default()).unwrap().len(),
        1,
        "reads keep answering from the last good load"
    );

    handle.shutdown();
}

/// Discovery hands the actor a canonical root; the actor must accept a project
/// whose inventory lives somewhere other than `opys/`.
#[test]
fn corpus_base_comes_from_the_project_config() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let corpus = fixture(&root);
    assert_eq!(corpus.base, PathBuf::from(&root).join("inventory"));
    let (handle, _rx) = spawn(corpus);
    assert_eq!(handle.docs(DocFilter::default()).unwrap().len(), 1);
    handle.shutdown();
}
