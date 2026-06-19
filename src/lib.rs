//! opys — a file-based inventory of typed markdown documents: one markdown file
//! per document, with YAML frontmatter, stable IDs, tags, configurable types,
//! test plans, and a `verify` gate for CI.
//!
//! The binary is a thin wrapper around [`run`]. The modules are public so the
//! crate can be used as a library.

pub mod body;
pub mod cli;
pub mod commands;
pub mod config;
pub mod doc;
pub mod error;
pub mod frontmatter;
pub mod links;
pub mod palette;
pub mod project;
pub mod project_config;
pub mod refs;
pub mod rules;
pub mod templates;
pub mod tui;

pub use error::{OpysError, Result};
pub use frontmatter::Frontmatter;
pub use project::Project;

use cli::{Cli, Command};

/// Shared invocation context: where the inventory lives and global flags.
pub struct Ctx {
    pub root: String,
    pub no_sync: bool,
}

impl Ctx {
    pub fn open(&self) -> Result<Project> {
        Project::open(&self.root)
    }
}

/// Execute a parsed CLI invocation, returning the process exit code.
///
/// `verify` returns `1` when it finds problems (the CI-gate contract); all
/// other commands return `0` on success and surface failures as
/// [`OpysError`], which the binary maps to exit code `2`.
pub fn run(cli: Cli) -> Result<i32> {
    let ctx = Ctx {
        root: cli.root,
        no_sync: cli.no_sync,
    };
    match cli.command {
        Command::Init => {
            commands::init::run(&ctx)?;
            Ok(0)
        }
        Command::New {
            type_name,
            title,
            tags,
            status,
            features,
            reason,
            field,
        } => {
            commands::new::run(
                &ctx,
                &type_name,
                &title,
                &tags,
                &status,
                &features,
                reason.as_deref(),
                &field,
            )?;
            Ok(0)
        }
        Command::Import { type_name, file } => {
            commands::import::run(&ctx, &type_name, &file)?;
            Ok(0)
        }
        Command::Show { id } => {
            commands::show::run(&ctx, &id)?;
            Ok(0)
        }
        Command::List {
            type_name,
            tag,
            status,
            field,
            format,
        } => {
            commands::list::run(
                &ctx,
                type_name.as_deref(),
                tag.as_deref(),
                status.as_deref(),
                &field,
                format,
            )?;
            Ok(0)
        }
        Command::SetStatus { id, status, reason } => {
            commands::set_status::run(&ctx, &id, &status, reason.as_deref())?;
            Ok(0)
        }
        Command::Tag { id, add, remove } => {
            commands::tag::run(&ctx, &id, add.as_deref(), remove.as_deref())?;
            Ok(0)
        }
        Command::Retire { id, reason } => {
            commands::retire::run(&ctx, &id, &reason)?;
            Ok(0)
        }
        Command::Block { id, by } => {
            commands::block::block(&ctx, &id, &by)?;
            Ok(0)
        }
        Command::Unblock { id, by } => {
            commands::block::unblock(&ctx, &id, &by)?;
            Ok(0)
        }
        Command::Verify => commands::verify::run(&ctx),
        Command::Sync => {
            commands::sync::run_command(&ctx)?;
            Ok(0)
        }
        Command::Stats => {
            commands::stats::run(&ctx)?;
            Ok(0)
        }
        #[cfg(feature = "history")]
        Command::History { id } => {
            commands::history::run(&ctx, &id)?;
            Ok(0)
        }
        Command::Close { id, force } => {
            commands::close::run(&ctx, &id, force)?;
            Ok(0)
        }
        Command::Cleanup => {
            commands::cleanup::run(&ctx)?;
            Ok(0)
        }
        Command::Config(cmd) => match cmd {
            cli::ConfigCommand::Init => {
                commands::config::init(&ctx)?;
                Ok(0)
            }
            cli::ConfigCommand::Validate => commands::config::validate(&ctx),
        },
        Command::AgentRules { tool, stdout } => {
            commands::agent_rules::run(&ctx, tool, stdout)?;
            Ok(0)
        }
        Command::Tui { dir } => {
            // A positional directory overrides the global `--root`.
            let ctx = match dir {
                Some(root) => Ctx {
                    root,
                    no_sync: ctx.no_sync,
                },
                None => ctx,
            };
            tui::run(&ctx)
        }
    }
}
