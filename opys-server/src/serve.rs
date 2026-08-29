//! Running the node: the one implementation both entry points use.
//!
//! `opys web start` and `opys-server run` are the same code — this module. The
//! two binaries differ only in how they were invoked; neither owns a copy of the
//! wiring, so the node cannot behave one way under one name and another way
//! under the other.
//!
//! Everything here is synchronous at the edges ([`blocking`]) so a plain
//! `fn main` can start the node without a tokio dependency of its own.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use opys_backend_markdown_local::MarkdownLocal;
use opys_engine::backend::Backend;
use opys_engine::error::{usage, OpysError, Result};
use tokio::sync::broadcast;

use crate::api::{self, AppState};
use crate::manager::Manager;
use crate::registry::{self, Registry};

/// Where the node listens when neither the flag nor the allowlist file says.
/// Loopback on purpose: there is no auth, and the bind address is the boundary.
pub const DEFAULT_BIND: &str = "127.0.0.1:6797";

/// The cheap pass: stat what is already served, react to the allowlist file.
const REFRESH_EVERY: Duration = Duration::from_secs(60);

/// The expensive pass: re-read and re-expand the allowlist. A prefix entry makes
/// this a bounded filesystem walk, so it runs rarely (ADR-0077 has the numbers).
const RESCAN_EVERY: Duration = Duration::from_secs(3600);

/// How many events the broadcast channel buffers for a client that is not
/// reading. Past this it is dropped rather than allowed to slow everyone down.
const EVENT_BUFFER: usize = 256;

/// Which allowlist file to use: the flag, else the XDG default.
///
/// Every `web` subcommand resolves it through here, so `add`, `list` and the
/// running node can never disagree about which file is the allowlist.
pub fn config_for(flag: Option<&Path>) -> Result<PathBuf> {
    match flag {
        Some(path) => Ok(path.to_path_buf()),
        None => registry::config_path(),
    }
}

/// A backend per corpus actor: each owns its own instance, and it has to cross a
/// thread boundary.
fn backend() -> Box<dyn Backend + Send> {
    Box::new(MarkdownLocal)
}

/// Where to listen: the flag wins, then the allowlist file's `bind`, then
/// [`DEFAULT_BIND`].
///
/// An unparseable `bind` in the file is fatal rather than ignored. Listening
/// somewhere other than where the user asked is the one mistake that could put
/// an unauthenticated API on a wider interface than intended — or hide it from
/// the client that expects it.
pub fn resolve_bind(flag: Option<SocketAddr>, from_file: Option<&str>) -> Result<SocketAddr> {
    if let Some(addr) = flag {
        return Ok(addr);
    }
    match from_file {
        Some(text) => text.parse().map_err(|e| {
            usage(format!(
                "bind = {text:?} in the allowlist file is not an address: {e}"
            ))
        }),
        None => Ok(DEFAULT_BIND
            .parse()
            .expect("the default bind address parses")),
    }
}

/// Where to listen, given the flag and the allowlist file.
///
/// A malformed allowlist is fatal here for the same reason it is in
/// [`Registry::load_from`]: serving less than the user asked for, quietly, is
/// worse than refusing to start.
pub fn bind_address(flag: Option<SocketAddr>, config: &Path) -> Result<SocketAddr> {
    let registry = Registry::load_from(config)?;
    resolve_bind(flag, registry.bind.as_deref())
}

/// Serve until the process ends, from a synchronous caller.
///
/// Owns the tokio runtime so the `opys` CLI — a plain `fn main` — can start the
/// node without linking tokio's macros or configuring a second runtime that
/// could drift from this one.
pub fn blocking(config: PathBuf, bind: SocketAddr) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run(config, bind))
}

/// Build the manager, serve, and keep the two periodic passes running.
pub async fn run(config: PathBuf, bind: SocketAddr) -> Result<()> {
    let (events, _) = broadcast::channel(EVENT_BUFFER);
    let manager = Arc::new(Mutex::new(Manager::new(
        config.clone(),
        events.clone(),
        backend,
    )));
    // Bind *before* the first rescan. Expanding a `[[prefix]]` entry walks the
    // filesystem, which ADR-0077 bounds but does not make free, and a startup
    // that scans first leaves the socket refusing connections for the length of
    // the walk — the exact shape that ADR refused. A malformed allowlist is
    // still fatal before this point: `bind_address` loaded and parsed the file
    // to find the port.
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| usage(format!("cannot bind {bind}: {e}")))?;
    eprintln!("opys-server listening on http://{bind}");

    // Reads before the first rescan lands see an empty node, not an error: no
    // project is a real state (a fresh install looks exactly like this), while a
    // 500 would be a lie about a node that is working. `/api/health` carries
    // `scanned` so a client can tell "nothing allowlisted" from "not yet
    // looked".
    let scanned = Arc::new(AtomicBool::new(false));
    tokio::spawn(initial_rescan(
        Arc::clone(&manager),
        config.clone(),
        Arc::clone(&scanned),
    ));

    // Both passes take `&mut Manager` and block — `rescan` joins actor threads —
    // so they live on the blocking pool, never on the reactor.
    tokio::spawn(ticker(
        Arc::clone(&manager),
        REFRESH_EVERY,
        "refresh",
        Manager::refresh,
    ));
    tokio::spawn(ticker(
        Arc::clone(&manager),
        RESCAN_EVERY,
        "rescan",
        Manager::rescan,
    ));

    // The state is told where it listens so it can refuse requests addressed to
    // some other name — a page the user visits cannot be stopped from calling a
    // loopback URL, only from being answered.
    let state = AppState::new(manager, events)
        .with_bind(bind)
        .with_scanned(scanned);
    axum::serve(listener, api::router(state))
        .await
        .map_err(OpysError::Io)
}

/// The first rescan, off the reactor and after the socket is up.
///
/// A failure here is logged, not fatal: the node is already serving, and the
/// periodic rescan will try again. Startup used to propagate this, but that was
/// only reachable when the rescan ran before `bind`.
async fn initial_rescan(manager: Arc<Mutex<Manager>>, config: PathBuf, scanned: Arc<AtomicBool>) {
    let result = tokio::task::spawn_blocking(move || rescan_and_report(&manager, &config)).await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => eprintln!("warning: startup rescan failed: {e}"),
        Err(e) => eprintln!("warning: startup rescan task failed: {e}"),
    }
    // Set either way: the question it answers is "has the first pass run", not
    // "did it succeed". A client that never saw this flip would wait forever.
    scanned.store(true, Ordering::Release);
}

/// The startup rescan, plus a line about what came of it.
///
/// An empty allowlist is a normal state — it is what a fresh install looks like
/// — so it is reported, not treated as a failure.
fn rescan_and_report(manager: &Arc<Mutex<Manager>>, config: &Path) -> Result<()> {
    let mut mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
    mgr.rescan()?;
    match mgr.len() {
        0 => eprintln!(
            "opys-server: nothing allowlisted in {} — no project is served until one is added",
            config.display()
        ),
        1 => eprintln!("opys-server: serving 1 corpus from {}", config.display()),
        n => eprintln!("opys-server: serving {n} corpora from {}", config.display()),
    }
    Ok(())
}

/// One of the manager's periodic passes.
type Pass = fn(&mut Manager) -> Result<()>;

/// Run `pass` every `every`, forever.
///
/// A failing tick is logged and the loop continues: an allowlist caught halfway
/// through an editor's save must not take the node down, and the next tick will
/// read the finished file.
async fn ticker(manager: Arc<Mutex<Manager>>, every: Duration, name: &'static str, pass: Pass) {
    let mut interval = tokio::time::interval(every);
    // The first tick fires immediately, and startup has already rescanned.
    interval.tick().await;
    loop {
        interval.tick().await;
        let manager = Arc::clone(&manager);
        let result = tokio::task::spawn_blocking(move || {
            let mut mgr = manager.lock().unwrap_or_else(|e| e.into_inner());
            pass(&mut mgr)
        })
        .await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => eprintln!("warning: {name} failed: {e}"),
            Err(e) => eprintln!("warning: {name} task failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_prefers_the_flag_then_the_file_then_the_default() {
        let flag: SocketAddr = "10.0.0.1:1".parse().unwrap();
        assert_eq!(
            resolve_bind(Some(flag), Some("127.0.0.1:9999")).unwrap(),
            flag,
            "the flag wins"
        );
        assert_eq!(
            resolve_bind(None, Some("127.0.0.1:9999"))
                .unwrap()
                .to_string(),
            "127.0.0.1:9999"
        );
        assert_eq!(resolve_bind(None, None).unwrap().to_string(), DEFAULT_BIND);
        assert!(
            resolve_bind(None, Some("not-an-address")).is_err(),
            "a bad address must not silently fall back"
        );
    }

    #[test]
    fn config_for_prefers_the_flag() {
        let flag = PathBuf::from("/tmp/somewhere/server.toml");
        assert_eq!(config_for(Some(&flag)).unwrap(), flag);
    }

    #[test]
    fn bind_address_reads_the_allowlist_file() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("server.toml");
        std::fs::write(&config, "bind = \"0.0.0.0:1234\"\n").unwrap();
        assert_eq!(
            bind_address(None, &config).unwrap().to_string(),
            "0.0.0.0:1234"
        );
        // A missing file is an empty allowlist, so the default stands.
        assert_eq!(
            bind_address(None, &tmp.path().join("absent.toml"))
                .unwrap()
                .to_string(),
            DEFAULT_BIND
        );
    }
}
