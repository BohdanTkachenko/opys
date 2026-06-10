//! opys — a file-based feature inventory: one markdown file per feature, with
//! YAML frontmatter, stable IDs, tags, test plans, manual-verification
//! runbooks, and a `verify` gate for CI.
//!
//! The binary is a thin wrapper around [`run`]. The modules are public so the
//! crate can be used as a library.

pub mod body;
pub mod cli;
pub mod commands;
pub mod config;
pub mod error;
pub mod feature;
pub mod frontmatter;
pub mod project;
pub mod templates;

pub use config::Config;
pub use error::{OpysError, Result};
pub use feature::Feature;
pub use frontmatter::Frontmatter;
pub use project::Project;

use cli::{Cli, Command};

/// Execute a parsed CLI invocation, returning the process exit code.
///
/// `verify` returns `1` when it finds problems (the CI-gate contract); all
/// other commands return `0` on success and surface failures as
/// [`OpysError`], which the binary maps to exit code `2`.
pub fn run(cli: Cli) -> Result<i32> {
    let root = &cli.root;
    match cli.command {
        Command::Init => {
            commands::init::run(root)?;
            Ok(0)
        }
        Command::New {
            title,
            tags,
            status,
            field,
        } => {
            commands::new::run(root, &title, &tags, &status, &field)?;
            Ok(0)
        }
        Command::Show { id } => {
            commands::show::run(root, &id)?;
            Ok(0)
        }
        Command::List {
            tag,
            status,
            format,
        } => {
            commands::list::run(root, tag.as_deref(), status.as_deref(), format)?;
            Ok(0)
        }
        Command::SetStatus { id, status, reason } => {
            commands::set_status::run(root, &id, &status, reason.as_deref())?;
            Ok(0)
        }
        Command::Tag { id, add, remove } => {
            commands::tag::run(root, &id, add.as_deref(), remove.as_deref())?;
            Ok(0)
        }
        Command::Retire { id, reason } => {
            commands::retire::run(root, &id, &reason)?;
            Ok(0)
        }
        Command::Verify => commands::verify::run(root),
        Command::SyncViews => {
            commands::sync_views::run(root)?;
            Ok(0)
        }
        Command::Report => {
            commands::report::run(root)?;
            Ok(0)
        }
        Command::ManualRunbook { out, name } => {
            commands::runbook::run(root, out.as_deref(), name.as_deref())?;
            Ok(0)
        }
    }
}
