//! The event stream, end to end over a real socket (TASK-0071).
//!
//! Everything else about the API can be driven through the router; this cannot.
//! The upgrade, the hello frame, and the fan-out from a filesystem change are
//! only meaningful with a client on the other end of a TCP connection.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use opys_backend_markdown_local::MarkdownLocal;
use opys_engine::backend::Backend;
use opys_server::actor::DocFilter;
use opys_server::api::{self, AppState};
use opys_server::manager::Manager;
use serde_json::Value;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const CONFIG: &str = r#"
base = "inventory"

[types.note]
prefix = "NOTE"
statuses = ["open", "closed"]
default_status = "open"
terminal_statuses = ["closed"]
tags_required = false
"#;

type Client = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// The ping interval the liveness tests run at. The production constant is 30 s
/// with two missed pings allowed, which is a minute and a half of wall clock per
/// assertion; the logic is the same at a hundredth of the scale.
const PING_EVERY: Duration = Duration::from_millis(200);

fn backend() -> Box<dyn Backend + Send> {
    Box::new(MarkdownLocal)
}

fn write_note(inventory: &Path, n: u32) {
    std::fs::write(
        inventory.join(format!("NOTE-{n:04}.md")),
        format!("---\nid: NOTE-{n:04}\nstatus: open\n---\n\n# Note {n}\n\nBody.\n"),
    )
    .unwrap();
}

fn project(root: &Path) -> PathBuf {
    let inventory = root.join("inventory");
    std::fs::create_dir_all(&inventory).unwrap();
    std::fs::write(root.join("opys.toml"), CONFIG).unwrap();
    write_note(&inventory, 1);
    std::fs::canonicalize(root).unwrap()
}

/// The next text frame, parsed. Anything else — a server ping, say — is skipped.
async fn next_json(socket: &mut Client) -> Option<Value> {
    while let Some(frame) = socket.next().await {
        match frame.ok()? {
            Message::Text(text) => return serde_json::from_str(text.as_str()).ok(),
            Message::Close(_) => return None,
            _ => {}
        }
    }
    None
}

/// A node over `root`, serving on a loopback port. `ping` shortens the
/// WebSocket liveness interval so the drop path is testable in under a second
/// instead of the ninety the production constants imply.
async fn serve(config: PathBuf, ping: Option<Duration>) -> (SocketAddr, String) {
    let (events, _) = broadcast::channel(32);
    let mut manager = Manager::new(config, events.clone(), backend);
    manager.rescan().unwrap();
    let cid = manager.cids().pop().expect("one corpus");
    // A read blocks until the actor has finished its startup load, so an event a
    // test waits for can only be a later one — not the one the actor broadcast
    // while it was starting.
    assert_eq!(
        manager
            .get(&cid)
            .unwrap()
            .docs(DocFilter::default())
            .unwrap()
            .len(),
        1
    );

    let mut state = AppState::new(Arc::new(Mutex::new(manager)), events);
    if let Some(every) = ping {
        state = state.with_ping_interval(every);
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, api::router(state)).await.unwrap();
    });
    (addr, cid)
}

fn allowlist(dir: &Path, root: &Path) -> PathBuf {
    let config = dir.join("config/server.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        format!("[[project]]\npath = {:?}\n", root.display().to_string()),
    )
    .unwrap();
    config
}

async fn connect(addr: SocketAddr) -> Client {
    let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/events"))
        .await
        .expect("the upgrade succeeds");
    socket
}

#[tokio::test(flavor = "multi_thread")]
async fn an_external_edit_reaches_a_websocket_client() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(&dir.path().join("proj"));
    let config = allowlist(dir.path(), &root);
    let (addr, cid) = serve(config, None).await;

    let mut socket = connect(addr).await;
    let hello = next_json(&mut socket).await.expect("a hello frame");
    assert_eq!(hello["type"], "hello", "{hello}");
    assert_eq!(hello["version"], env!("CARGO_PKG_VERSION"));

    // An edit nobody told the server about.
    write_note(&root.join("inventory"), 2);

    let event = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let frame = next_json(&mut socket).await.expect("the socket stays open");
            if frame["event"] == "corpus-reloaded" {
                return frame;
            }
        }
    })
    .await
    .expect("a corpus-reloaded frame within 3s");

    assert_eq!(event["cid"], cid.as_str());
    assert_eq!(
        event["docs"], 2,
        "the payload carries the new count: {event}"
    );
    assert_eq!(event["verify_problems"], 0);
    assert!(event["ts"].is_string(), "{event}");
}

/// The liveness contract, at a scale a test can afford: a client that answers
/// pings keeps its stream, and keeps receiving events over it.
///
/// Without this, dropping the `unanswered = 0` reset on a pong — which would
/// disconnect every idle browser tab a minute and a half after it opened — is
/// invisible to the suite.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_answers_pings_keeps_its_stream() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(&dir.path().join("proj"));
    let config = allowlist(dir.path(), &root);
    let (addr, _cid) = serve(config, Some(PING_EVERY)).await;

    let mut socket = connect(addr).await;
    assert_eq!(
        next_json(&mut socket).await.expect("a hello frame")["type"],
        "hello"
    );

    // Several ping cycles with nothing to say. Reading the socket is what makes
    // tungstenite answer the pings, which is exactly what a browser does.
    let idle = tokio::time::timeout(PING_EVERY * 6, next_json(&mut socket)).await;
    assert!(
        idle.is_err(),
        "an idle client should be pinged, not closed: {idle:?}"
    );

    // …and the stream still works.
    write_note(&root.join("inventory"), 2);
    let event = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let frame = next_json(&mut socket).await.expect("the socket stays open");
            if frame["event"] == "corpus-reloaded" {
                return frame;
            }
        }
    })
    .await
    .expect("a corpus-reloaded frame within 3s");
    assert_eq!(event["docs"], 2, "{event}");
}

/// …and a client that stops reading is dropped rather than kept forever. A dead
/// tab holds a task, a socket and a broadcast receiver until the process exits.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_never_answers_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let root = project(&dir.path().join("proj"));
    let config = allowlist(dir.path(), &root);
    let (addr, _cid) = serve(config, Some(PING_EVERY)).await;

    let mut socket = connect(addr).await;
    assert_eq!(
        next_json(&mut socket).await.expect("a hello frame")["type"],
        "hello"
    );

    // Two pings go unanswered because nothing is polling the socket; the third
    // tick gives up on it.
    tokio::time::sleep(PING_EVERY * 4).await;

    let closed = tokio::time::timeout(Duration::from_secs(3), async {
        // The buffered pings arrive first; what matters is that the stream ends.
        while let Some(frame) = socket.next().await {
            match frame {
                Ok(Message::Close(_)) | Err(_) => return true,
                Ok(_) => {}
            }
        }
        true
    })
    .await;
    assert_eq!(
        closed.ok(),
        Some(true),
        "a peer that never answers a ping must be dropped"
    );
}
