//! `opys-server` — the always-on opys node (FEAT-0058).
//!
//! A long-lived process that serves the projects the user has allowlisted: one
//! warm store per corpus, the typed opys API, and the event stream. It takes no
//! project paths on the command line — the allowlist file is the only way in
//! (ADR-0077), so approving a project and running the node are separate acts.
//!
//! Apache-2.0 like the rest of the workspace: everything that runs on the user's
//! own machine is permissive, and the copyleft boundary starts at the relay
//! (ADR-0076).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use opys_backend_markdown_local::MarkdownLocal;
use opys_engine::backend::Backend;
use opys_server::api::{self, AppState};
use opys_server::manager::Manager;
use opys_server::registry::{self, Registry};
use tokio::sync::broadcast;

/// Where the node listens when neither the flag nor the allowlist file says.
/// Loopback on purpose: there is no auth, and the bind address is the boundary.
const DEFAULT_BIND: &str = "127.0.0.1:6797";

/// The cheap pass: stat what is already served, react to the allowlist file.
const REFRESH_EVERY: Duration = Duration::from_secs(60);

/// The expensive pass: re-read and re-expand the allowlist. A prefix entry makes
/// this a bounded filesystem walk, so it runs rarely (ADR-0077 has the numbers).
const RESCAN_EVERY: Duration = Duration::from_secs(3600);

/// How many events the broadcast channel buffers for a client that is not
/// reading. Past this it is dropped rather than allowed to slow everyone down.
const EVENT_BUFFER: usize = 256;

#[derive(Parser)]
#[command(
    name = "opys-server",
    version,
    about = "Always-on opys node: watcher, HTTP/WS API, and web UI over local inventories",
    // `run` is the default: `opys-server --bind …` == `opys-server run --bind …`.
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(flatten)]
    run: RunArgs,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the API and web UI over the allowlisted projects.
    Run(RunArgs),

    /// Install the user systemd unit.
    Install,

    /// Remove the user systemd unit.
    Uninstall,
}

#[derive(Args)]
struct RunArgs {
    /// Address to listen on. Overrides `bind` in the allowlist file; the default
    /// is 127.0.0.1:6797.
    #[arg(long)]
    bind: Option<SocketAddr>,

    /// Allowlist file to serve from, instead of the default
    /// `$XDG_CONFIG_HOME/opys/server.toml`.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

/// A backend per corpus actor: each owns its own instance, and it has to cross a
/// thread boundary.
fn backend() -> Box<dyn Backend + Send> {
    Box::new(MarkdownLocal)
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Some(Command::Run(args)) => serve(args).await,
        None => serve(cli.run).await,
        Some(Command::Install | Command::Uninstall) => {
            eprintln!("error: not implemented yet (TASK-0075)");
            2
        }
    };
    std::process::exit(code);
}

/// Where to listen: the flag wins, then the allowlist file's `bind`, then
/// [`DEFAULT_BIND`].
///
/// An unparseable `bind` in the file is fatal rather than ignored. Listening
/// somewhere other than where the user asked is the one mistake that could put
/// an unauthenticated API on a wider interface than intended — or hide it from
/// the client that expects it.
fn resolve_bind(flag: Option<SocketAddr>, from_file: Option<&str>) -> Result<SocketAddr, String> {
    if let Some(addr) = flag {
        return Ok(addr);
    }
    match from_file {
        Some(text) => text
            .parse()
            .map_err(|e| format!("bind = {text:?} in the allowlist file is not an address: {e}")),
        None => Ok(DEFAULT_BIND
            .parse()
            .expect("the default bind address parses")),
    }
}

/// Where to listen, given the flag and the allowlist file.
///
/// A malformed allowlist is fatal here for the same reason it is in
/// `Registry::load_from`: serving less than the user asked for, quietly, is
/// worse than refusing to start.
fn bind_address(flag: Option<SocketAddr>, config: &Path) -> Result<SocketAddr, String> {
    let registry = Registry::load_from(config).map_err(|e| e.to_string())?;
    resolve_bind(flag, registry.bind.as_deref())
}

/// The exit code for a `run`: everything that goes wrong before or during
/// serving is a hard failure, reported once, here.
async fn serve(args: RunArgs) -> i32 {
    match run(args).await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            2
        }
    }
}

/// Build the manager, serve, and keep the two periodic passes running.
async fn run(args: RunArgs) -> Result<(), String> {
    let config = match args.config {
        Some(path) => path,
        None => registry::config_path().map_err(|e| e.to_string())?,
    };
    let bind = bind_address(args.bind, &config)?;

    let (events, _) = broadcast::channel(EVENT_BUFFER);
    let manager = Arc::new(Mutex::new(Manager::new(
        config.clone(),
        events.clone(),
        backend,
    )));
    rescan_and_report(&manager, &config).map_err(|e| e.to_string())?;

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

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| format!("cannot bind {bind}: {e}"))?;
    eprintln!("opys-server listening on http://{bind}");
    // The state is told where it listens so it can refuse requests addressed to
    // some other name — a page the user visits cannot be stopped from calling a
    // loopback URL, only from being answered.
    let state = AppState::new(manager, events).with_bind(bind);
    axum::serve(listener, api::router(state))
        .await
        .map_err(|e| e.to_string())
}

/// The startup rescan, plus a line about what came of it.
///
/// An empty allowlist is a normal state — it is what a fresh install looks like
/// — so it is reported, not treated as a failure.
fn rescan_and_report(
    manager: &Arc<Mutex<Manager>>,
    config: &Path,
) -> opys_engine::error::Result<()> {
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
type Pass = fn(&mut Manager) -> opys_engine::error::Result<()>;

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
    fn run_is_the_default_subcommand() {
        let cli = Cli::parse_from(["opys-server"]);
        assert!(cli.command.is_none());
        assert!(cli.run.bind.is_none(), "unset means the resolved default");
        assert!(cli.run.config.is_none());
    }

    /// ADR-0077: the node serves the allowlist, so there are no project
    /// arguments to give it — only where to listen and which allowlist to read.
    #[test]
    fn run_takes_no_roots_only_bind_and_config() {
        let cli = Cli::parse_from([
            "opys-server",
            "run",
            "--bind",
            "0.0.0.0:1234",
            "--config",
            "/tmp/server.toml",
        ]);
        let Some(Command::Run(args)) = cli.command else {
            panic!("expected the run subcommand");
        };
        assert_eq!(
            args.bind.map(|b| b.to_string()).as_deref(),
            Some("0.0.0.0:1234")
        );
        assert_eq!(args.config, Some(PathBuf::from("/tmp/server.toml")));

        assert!(
            Cli::try_parse_from(["opys-server", "/tmp/a"]).is_err(),
            "a project path is not an argument any more"
        );
        // …and the stub subcommands still parse.
        assert!(Cli::try_parse_from(["opys-server", "install"]).is_ok());
    }

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
}
