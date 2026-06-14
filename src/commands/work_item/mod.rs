//! `opys work-item …` subcommands.

pub mod cleanup;
pub mod close;
pub mod init;
pub mod list;
pub mod new;
pub mod set_status;
pub mod show;
pub mod tag;

use crate::cli::WorkItemCommand;
use crate::error::Result;
use crate::Ctx;

pub fn run(ctx: &Ctx, cmd: WorkItemCommand) -> Result<()> {
    match cmd {
        WorkItemCommand::Init => init::run(ctx),
        WorkItemCommand::New {
            title,
            features,
            status,
            tags,
            reason,
            field,
        } => new::run(
            ctx,
            &title,
            &features,
            &status,
            &tags,
            reason.as_deref(),
            &field,
        ),
        WorkItemCommand::Show { id } => show::run(ctx, &id),
        WorkItemCommand::List {
            feature,
            status,
            format,
        } => list::run(ctx, feature.as_deref(), status.as_deref(), format),
        WorkItemCommand::SetStatus { id, status, reason } => {
            set_status::run(ctx, &id, &status, reason.as_deref())
        }
        WorkItemCommand::Tag { id, add, remove } => {
            tag::run(ctx, &id, add.as_deref(), remove.as_deref())
        }
        WorkItemCommand::Close { id, force } => close::run(ctx, &id, force),
        WorkItemCommand::Cleanup => cleanup::run(ctx),
    }
}
