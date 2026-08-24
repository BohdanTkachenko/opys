//! `opys-server` — the always-on opys node (FEAT-0058).
//!
//! A long-lived process that serves the projects the user has allowlisted: one
//! warm store per corpus, the typed opys API, and the event stream. It takes no
//! project paths on the command line — the allowlist file is the only way in
//! (ADR-0077), so approving a project and running the node are separate acts.
//!
//! This binary is argument plumbing only. Every subcommand it offers is the
//! `web` surface from [`opys_server::cli`], the same one the `opys` CLI mounts,
//! so `opys web start` and `opys-server run` are one implementation — and the
//! `ExecStart=<exe> web start …` line the systemd installer writes is valid
//! whichever of the two binaries wrote it.
//!
//! Apache-2.0 like the rest of the workspace: everything that runs on the user's
//! own machine is permissive, and the copyleft boundary starts at the relay
//! (ADR-0076).

use clap::{Parser, Subcommand};
use opys_server::cli::{dispatch, InstallArgs, StartArgs, WebCommand};

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
    run: StartArgs,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the API and web UI over the allowlisted projects.
    Run(StartArgs),

    /// Manage the node: the same surface as `opys web`.
    Web {
        #[command(subcommand)]
        command: WebCommand,
    },

    /// Write the systemd user unit (shorthand for `web install`).
    Install(InstallArgs),

    /// Remove the systemd user unit (shorthand for `web uninstall`).
    Uninstall,
}

fn main() {
    let cli = Cli::parse();
    let command = match cli.command {
        None => WebCommand::Start(cli.run),
        Some(Command::Run(args)) => WebCommand::Start(args),
        Some(Command::Web { command }) => command,
        Some(Command::Install(args)) => WebCommand::Install(args),
        Some(Command::Uninstall) => WebCommand::Uninstall,
    };
    match dispatch(command) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn run_is_the_default_subcommand() {
        let cli = Cli::parse_from(["opys-server"]);
        assert!(cli.command.is_none());
        assert!(cli.run.bind.is_none(), "unset means the resolved default");
        assert!(cli.run.config.is_none());
    }

    /// `StartArgs` is *flattened* here, and a flattened `Args`'s doc comment
    /// replaces the root command's own description unless it is reset — which
    /// silently turns `opys-server --help` into a paragraph about `--bind`.
    #[test]
    fn the_flattened_start_args_do_not_take_over_the_help_header() {
        use clap::CommandFactory;
        assert_eq!(
            Cli::command()
                .get_about()
                .map(ToString::to_string)
                .as_deref(),
            Some("Always-on opys node: watcher, HTTP/WS API, and web UI over local inventories")
        );
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
    }

    /// The unit's `ExecStart` says `<exe> web start`, so this binary has to
    /// understand that form too.
    #[test]
    fn the_web_surface_is_mounted_here_as_well() {
        let cli = Cli::parse_from(["opys-server", "web", "start", "--bind", "0.0.0.0:2"]);
        assert!(matches!(
            cli.command,
            Some(Command::Web {
                command: WebCommand::Start(_)
            })
        ));
        assert!(Cli::try_parse_from(["opys-server", "web", "list"]).is_ok());
        // …and the install shorthands still parse.
        assert!(Cli::try_parse_from(["opys-server", "install"]).is_ok());
        assert!(Cli::try_parse_from(["opys-server", "uninstall"]).is_ok());
    }
}
