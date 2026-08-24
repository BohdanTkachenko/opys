//! The manager keeping live corpus actors in step with the allowlist
//! (TASK-0070, ADR-0077).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use opys_backend_markdown_local::MarkdownLocal;
use opys_engine::backend::Backend;
use opys_server::actor::{DocFilter, Event};
use opys_server::manager::Manager;
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

fn backend() -> Box<dyn Backend + Send> {
    Box::new(MarkdownLocal)
}

/// A project with one note.
fn project(root: &Path, n: u32) -> PathBuf {
    let inventory = root.join("inventory");
    std::fs::create_dir_all(&inventory).unwrap();
    std::fs::write(root.join("opys.toml"), CONFIG).unwrap();
    std::fs::write(
        inventory.join(format!("NOTE-{n:04}.md")),
        format!("---\nid: NOTE-{n:04}\nstatus: open\n---\n\n# Note {n}\n\nBody.\n"),
    )
    .unwrap();
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

fn drain(rx: &mut broadcast::Receiver<Event>) -> Vec<Event> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

/// Wait until `pred` holds over the drained events, so an assertion never races
/// an actor thread that has not published yet.
fn wait_for(rx: &mut broadcast::Receiver<Event>, pred: impl Fn(&Event) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if drain(rx).iter().any(&pred) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn an_empty_allowlist_serves_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config/server.toml");
    let (tx, _rx) = broadcast::channel(32);

    let mut mgr = Manager::new(config, tx, backend);
    mgr.rescan().unwrap();
    assert!(mgr.is_empty(), "a fresh install serves nothing until asked");
    assert!(mgr.groups().is_empty());
    mgr.shutdown();
}

#[test]
fn rescan_starts_and_stops_actors_from_the_allowlist() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = project(&tmp.path().join("proj"), 1);
    let config = tmp.path().join("config/server.toml");
    write_allowlist(&config, &[&proj]);

    let (tx, mut rx) = broadcast::channel(32);
    let mut mgr = Manager::new(config.clone(), tx, backend);
    mgr.rescan().unwrap();

    assert_eq!(mgr.len(), 1);
    assert_eq!(mgr.groups().len(), 1);
    let cid = mgr.cids().into_iter().next().unwrap();
    let handle = mgr.get(&cid).expect("the corpus is served");
    assert_eq!(handle.docs(DocFilter::default()).unwrap().len(), 1);
    assert!(wait_for(&mut rx, |e| matches!(
        e,
        Event::CorpusAdded { .. }
    )));

    // Removed from the allowlist: the actor goes away with it.
    write_allowlist(&config, &[]);
    mgr.rescan().unwrap();
    assert!(mgr.is_empty(), "an unlisted project must stop being served");
    assert!(mgr.groups().is_empty());
    assert!(mgr.get(&cid).is_none());
    assert!(wait_for(&mut rx, |e| matches!(
        e,
        Event::CorpusRemoved { .. }
    )));
    mgr.shutdown();
}

/// `opys web add` edits the file and nothing else, so the manager has to notice
/// the file itself — this is the whole mechanism behind ADR-0077's rule that no
/// endpoint accepts a path.
#[test]
fn refresh_picks_up_an_edited_allowlist() {
    let tmp = tempfile::tempdir().unwrap();
    let one = project(&tmp.path().join("one"), 1);
    let two = project(&tmp.path().join("two"), 2);
    let config = tmp.path().join("config/server.toml");
    write_allowlist(&config, &[&one]);

    let (tx, mut rx) = broadcast::channel(32);
    let mut mgr = Manager::new(config.clone(), tx, backend);
    mgr.rescan().unwrap();
    assert_eq!(mgr.len(), 1);
    let _ = drain(&mut rx);

    // A second project is allowlisted behind the manager's back.
    write_allowlist(&config, &[&one, &two]);
    mgr.refresh().unwrap();

    assert_eq!(mgr.len(), 2, "the cheap tick must still react to the file");
    assert!(wait_for(&mut rx, |e| matches!(
        e,
        Event::CorpusAdded { .. }
    )));
    mgr.shutdown();
}

/// The frequent tick does no walking, but it does notice that something it is
/// already serving has gone.
#[test]
fn refresh_drops_a_corpus_whose_project_vanished() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = project(&tmp.path().join("proj"), 1);
    let config = tmp.path().join("config/server.toml");
    write_allowlist(&config, &[&proj]);

    let (tx, mut rx) = broadcast::channel(32);
    let mut mgr = Manager::new(config, tx, backend);
    mgr.rescan().unwrap();
    assert_eq!(mgr.len(), 1);
    // Wait out the startup load before deleting anything. `rescan` returns once
    // the actor thread is spawned, and the load it then runs takes the inventory
    // lock — which creates the inventory directory if it is missing. Racing that
    // against the removal below either loses a `rmdir` to a concurrent `mkdir`
    // (`DirectoryNotEmpty`) or resurrects the project the test just deleted. A
    // read blocks until the load is done, which orders the two for good.
    let cid = mgr.cids().into_iter().next().unwrap();
    mgr.get(&cid)
        .expect("the corpus is served")
        .docs(DocFilter::default())
        .unwrap();
    let _ = drain(&mut rx);

    std::fs::remove_dir_all(&proj).unwrap();
    mgr.refresh().unwrap();

    assert!(mgr.is_empty(), "a project that is gone stops being served");
    assert!(mgr.groups().is_empty(), "and stops being listed");
    assert!(wait_for(&mut rx, |e| matches!(
        e,
        Event::CorpusRemoved { .. }
    )));
    mgr.shutdown();
}

/// A quiet tick must not churn: no restarts, no events, and the same actor
/// (hence the same warm store) still answering.
#[test]
fn a_quiet_refresh_changes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = project(&tmp.path().join("proj"), 1);
    let config = tmp.path().join("config/server.toml");
    write_allowlist(&config, &[&proj]);

    let (tx, mut rx) = broadcast::channel(32);
    let mut mgr = Manager::new(config, tx, backend);
    mgr.rescan().unwrap();
    let cids = mgr.cids();
    // `rescan` returns once the actor threads are spawned, not once they have
    // loaded. A read blocks until the startup load is done, and the actor
    // broadcasts that load before it serves anything — so draining after this
    // is guaranteed to clear the startup event instead of racing it into the
    // window below, where it would read as churn.
    for cid in &cids {
        mgr.get(cid)
            .expect("the corpus is served")
            .docs(DocFilter::default())
            .unwrap();
    }
    let _ = drain(&mut rx);

    for _ in 0..3 {
        mgr.refresh().unwrap();
    }

    assert_eq!(mgr.cids(), cids, "the same corpora, the same actors");
    let events = drain(&mut rx);
    assert!(
        events.is_empty(),
        "an idle tick should be silent, got {events:?}"
    );
    mgr.shutdown();
}
