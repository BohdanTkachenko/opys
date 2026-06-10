//! Subcommand implementations. Each `run` takes the project root plus its
//! parsed arguments.

pub mod init;
pub mod list;
pub mod new;
pub mod report;
pub mod retire;
pub mod runbook;
pub mod set_status;
pub mod show;
pub mod sync_views;
pub mod tag;
pub mod verify;

use time::{format_description::FormatItem, macros::format_description, OffsetDateTime};

const ISO_DATE: &[FormatItem<'static>] = format_description!("[year]-[month]-[day]");

/// Today's date as `YYYY-MM-DD` (local time, falling back to UTC).
pub fn today() -> String {
    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    now.format(&ISO_DATE).expect("ISO date formatting")
}

/// Split a comma-separated argument, trimming and dropping empties.
pub fn split_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}
