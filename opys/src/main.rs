//! The `opys` binary.
//!
//! Two things live here and nothing else: the top-level parser, which is the
//! engine's own command set plus `web`, and the dispatch that hands each half to
//! its crate. The inventory commands go to [`opys_engine::run`] untouched; `web`
//! goes to [`opys_server::cli::dispatch`], the same implementation the
//! `opys-server` binary mounts (ADR-0077).
//!
//! `opys-engine` deliberately knows nothing about the node — it is the library
//! every consumer embeds, and it does not pull in axum or tokio. Joining the two
//! surfaces is this binary's job.

use clap::{Parser, Subcommand};

use opys_backend_markdown_local::MarkdownLocal;

#[derive(Parser)]
#[command(
    name = "opys",
    version,
    about = "File-based inventory of typed markdown documents"
)]
struct Cli {
    /// Where to start searching upward for `opys.toml` (the project root).
    /// Defaults to the current directory.
    #[arg(long, default_value = ".", global = true)]
    pub root: String,

    /// Skip the automatic sync (reconcile/linkify/relocate) after mutating commands.
    #[arg(long, global = true)]
    pub no_sync: bool,

    #[command(subcommand)]
    pub command: TopCommand,
}

/// Everything `opys` can do: the engine's commands, flattened in so they keep
/// their exact names, help and order, plus the node.
#[derive(Subcommand)]
enum TopCommand {
    #[command(flatten)]
    Engine(opys_engine::cli::Command),

    /// The always-on node: serve the allowlisted projects over HTTP.
    Web {
        #[command(subcommand)]
        command: opys_server::cli::WebCommand,
    },
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        TopCommand::Engine(command) => opys_engine::run(
            opys_engine::cli::Cli {
                root: cli.root,
                no_sync: cli.no_sync,
                command,
            },
            Box::new(MarkdownLocal),
        ),
        TopCommand::Web { command } => {
            // clap propagates a global into every subcommand, `web` included,
            // where neither flag means anything — the node serves an allowlist,
            // not a project root. Refusing, rather than warning: `opys web scan
            // --root ~/work` is the spelling everyone reaches for first, and
            // ignoring it produces a confident, correct-looking scan of the
            // *home directory* instead. A warning above a long list of the wrong
            // projects is not a message anybody reads.
            if cli.root != "." || cli.no_sync {
                Err(opys_engine::error::usage(
                    "`--root` and `--no-sync` are inventory flags; `web` takes neither \
                     (the scan root is `opys web scan --under <PATH>`)",
                ))
            } else {
                opys_server::cli::dispatch(command)
            }
        }
    };
    match result {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    }
}

/// The price of owning a second `#[derive(Parser)]` is that the root metadata —
/// the name, the about line, the two global flags — now exists twice: here and
/// in `opys_engine::cli::Cli`. Nothing else in the build compares them, so a
/// future edit to one could quietly change `opys --help` without changing what
/// library embedders see (or the other way round).
///
/// These tests are that comparison. They also catch the other failure mode of
/// `#[command(flatten)]`: a name collision between an engine command and a
/// sibling variant, which clap only reports when it builds the command.
#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn subcommand_names(cmd: &clap::Command) -> Vec<String> {
        cmd.get_subcommands()
            .map(|c| c.get_name().to_string())
            .filter(|n| n != "help")
            .collect()
    }

    #[test]
    fn the_root_metadata_matches_the_engines() {
        let ours = Cli::command();
        let engine = opys_engine::cli::Cli::command();
        assert_eq!(ours.get_name(), engine.get_name());
        assert_eq!(
            ours.get_about().map(ToString::to_string),
            engine.get_about().map(ToString::to_string)
        );
        assert_eq!(
            ours.get_version().map(ToString::to_string),
            engine.get_version().map(ToString::to_string)
        );
    }

    #[test]
    fn the_global_flags_match_the_engines() {
        let ours = Cli::command();
        let engine = opys_engine::cli::Cli::command();
        let engine_args: Vec<&clap::Arg> = engine
            .get_arguments()
            .filter(|a| a.get_id() != "help" && a.get_id() != "version")
            .collect();
        assert!(!engine_args.is_empty(), "the engine has global flags");
        for expected in engine_args {
            let got = ours
                .get_arguments()
                .find(|a| a.get_id() == expected.get_id())
                .unwrap_or_else(|| panic!("`--{}` is missing here", expected.get_id()));
            assert_eq!(got.get_long(), expected.get_long());
            assert_eq!(
                got.get_help().map(ToString::to_string),
                expected.get_help().map(ToString::to_string),
                "help text for `--{}` drifted",
                expected.get_id()
            );
            assert_eq!(
                got.get_default_values(),
                expected.get_default_values(),
                "default for `--{}` drifted",
                expected.get_id()
            );
            assert_eq!(got.is_global_set(), expected.is_global_set());
        }
    }

    #[test]
    fn the_command_list_is_the_engines_plus_web() {
        let ours = subcommand_names(&Cli::command());
        let mut expected = subcommand_names(&opys_engine::cli::Cli::command());
        expected.push("web".to_string());
        assert_eq!(ours, expected);
    }

    /// `opys web` mounts the node's surface, and it needs no globals of its own.
    #[test]
    fn web_carries_the_node_surface() {
        let cli = Cli::parse_from(["opys", "web", "list"]);
        assert!(matches!(
            cli.command,
            TopCommand::Web {
                command: opys_server::cli::WebCommand::List { .. }
            }
        ));
        assert!(
            Cli::try_parse_from(["opys", "web"]).is_err(),
            "`web` on its own is not a command"
        );
    }

    /// The flattened half must keep parsing exactly as it did before `web`
    /// existed, globals and all, in either position.
    #[test]
    fn engine_commands_still_parse_with_globals_on_either_side() {
        let cli = Cli::parse_from(["opys", "--root", "/x", "list"]);
        assert_eq!(cli.root, "/x");
        assert!(matches!(
            cli.command,
            TopCommand::Engine(opys_engine::cli::Command::List { .. })
        ));

        let cli = Cli::parse_from(["opys", "verify", "--root", "/y"]);
        assert_eq!(cli.root, "/y");
        assert!(!cli.no_sync);

        let cli = Cli::parse_from(["opys", "sync", "--no-sync"]);
        assert!(cli.no_sync);
        assert_eq!(cli.root, ".", "the documented default");
    }
}
