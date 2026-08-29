//! Startup ordering: the socket is up before the filesystem is walked (BUG-0079).
//!
//! ADR-0077 bounds the discovery scan but does not make it free, and it is
//! explicit that scanning never blocks startup. Expanding a `[[prefix]]` entry
//! is what makes that concrete: the walk happens before anything can be served
//! unless `run` binds first. The old order refused connections for the length of
//! the walk, and every existing test builds the manager directly, so none of
//! them went through `serve::run` at all.

use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

/// A tree wide enough that walking it is measurable, but cheap to create.
///
/// The test needs the scan to still be running when the first request lands.
/// One prefix entry over a small tree could finish first on a fast machine with
/// a warm cache, so the allowlist points many entries at the same tree: the walk
/// is repeated per entry while the directories are created once.
const DIRS: usize = 400;
const ENTRIES: usize = 40;

fn build_tree(root: &Path) {
    for i in 0..DIRS {
        let d = root.join(format!("p{i:04}"));
        std::fs::create_dir_all(d.join("nested/deeper")).unwrap();
    }
}

/// A free port, released before the node takes it.
fn free_port() -> SocketAddr {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap()
}

#[tokio::test]
async fn the_socket_answers_while_the_first_scan_is_still_running() {
    let tmp = tempfile::tempdir().unwrap();
    let tree = tmp.path().join("tree");
    build_tree(&tree);

    let config = tmp.path().join("server.toml");
    let mut toml = String::new();
    for _ in 0..ENTRIES {
        toml.push_str(&format!(
            "[[prefix]]\npath = \"{}\"\ndepth = 10\n\n",
            tree.display()
        ));
    }
    std::fs::write(&config, toml).unwrap();

    let bind = free_port();
    tokio::spawn(opys_server::serve::run(config, bind));

    // Connect as soon as the listener exists. A generous ceiling: the point is
    // that this succeeds long before the walk could have finished, not that it
    // is instant.
    let start = Instant::now();
    let body = loop {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the node never accepted a connection"
        );
        match get(bind, "/api/health").await {
            Some(body) => break body,
            None => tokio::time::sleep(Duration::from_millis(2)).await,
        }
    };

    // Served, not merely accepted.
    let health: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(health["ok"], true, "got: {body}");

    // The window this bug is about: answering before the scan has finished. If
    // the walk had run first, `scanned` could never be observed false.
    assert_eq!(
        health["scanned"], false,
        "the first request was served after the scan finished — either the walk \
         is no longer slow enough to observe, or bind moved back behind it: {body}"
    );

    // And it does finish: an empty node that never fills in would be its own bug.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(Instant::now() < deadline, "the first scan never completed");
        if let Some(body) = get(bind, "/api/health").await {
            let v: serde_json::Value = serde_json::from_str(&body).unwrap();
            if v["scanned"] == true {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// One HTTP/1.1 GET over a raw socket, returning the body.
///
/// Hand-rolled rather than pulling a client crate in: the node's own test suite
/// has no HTTP client dependency, and this needs one request shape.
async fn get(addr: SocketAddr, path: &str) -> Option<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.ok()?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.ok()?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.ok()?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (_, body) = text.split_once("\r\n\r\n")?;
    Some(body.trim().to_string())
}
