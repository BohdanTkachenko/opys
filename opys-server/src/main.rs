//! `opys-server` — the always-on opys node (FEAT-0058).
//!
//! A long-lived process that watches project roots, holds a warm store per
//! corpus, and serves the typed opys API plus an embedded web UI. This is the
//! scaffold: argument parsing and a health route. Discovery, the corpus actors,
//! and the API arrive in the tasks that follow (TASK-0069 onward).
//!
//! Licensed AGPL-3.0-only, unlike the Apache-2.0 core it depends on (ADR-0056).

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::{routing::get, Json, Router};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;

#[derive(Parser)]
#[command(
    name = "opys-server",
    version,
    about = "Always-on opys node: watcher, HTTP/WS API, and web UI over local inventories",
    // `run` is the default: `opys-server ~/code` == `opys-server run ~/code`.
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
struct Cli {
    #[command(flatten)]
    run: RunArgs,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the API and web UI over the given project roots.
    Run(RunArgs),

    /// Install the user systemd unit.
    Install,

    /// Remove the user systemd unit.
    Uninstall,
}

#[derive(Args)]
struct RunArgs {
    /// Project roots to serve. Each is searched for `opys.toml` projects.
    #[arg(required = true)]
    roots: Vec<PathBuf>,

    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:6797")]
    bind: SocketAddr,
}

#[derive(Serialize)]
struct Health {
    ok: bool,
    version: &'static str,
}

async fn health() -> Json<Health> {
    Json(Health {
        ok: true,
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// The API router. Every route the server serves hangs off this one function so
/// tests can drive it without binding a socket.
fn router() -> Router {
    Router::new().route("/api/health", get(health))
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

async fn serve(args: RunArgs) -> i32 {
    let listener = match tokio::net::TcpListener::bind(args.bind).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: cannot bind {}: {e}", args.bind);
            return 2;
        }
    };
    eprintln!("opys-server listening on http://{}", args.bind);
    if let Err(e) = axum::serve(listener, router()).await {
        eprintln!("error: {e}");
        return 2;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn get_json(uri: &str) -> (StatusCode, serde_json::Value) {
        let res = router()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn health_reports_ok_and_version() {
        let (status, body) = get_json("/api/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn unknown_route_is_not_found() {
        let (status, _) = get_json("/api/nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn run_is_the_default_subcommand() {
        let cli = Cli::parse_from(["opys-server", "/tmp/a", "/tmp/b"]);
        assert!(cli.command.is_none());
        assert_eq!(cli.run.roots.len(), 2);
        assert_eq!(cli.run.bind.to_string(), "127.0.0.1:6797");
    }

    #[test]
    fn explicit_run_takes_roots_and_bind() {
        let cli = Cli::parse_from(["opys-server", "run", "--bind", "0.0.0.0:1234", "/tmp/a"]);
        let Some(Command::Run(args)) = cli.command else {
            panic!("expected the run subcommand");
        };
        assert_eq!(args.roots, [PathBuf::from("/tmp/a")]);
        assert_eq!(args.bind.to_string(), "0.0.0.0:1234");
    }

    #[test]
    fn run_requires_at_least_one_root() {
        assert!(Cli::try_parse_from(["opys-server"]).is_err());
        assert!(Cli::try_parse_from(["opys-server", "run"]).is_err());
        // …but the stub subcommands take none.
        assert!(Cli::try_parse_from(["opys-server", "install"]).is_ok());
    }
}
