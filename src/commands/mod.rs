//! Subcommand implementations. Each `run` takes the invocation [`Ctx`] plus
//! its parsed arguments.

pub mod agent_rules;
pub mod block;
pub mod cleanup;
pub mod close;
pub mod config;
pub mod import;
pub mod init;
pub mod list;
pub mod new;
pub mod retire;
pub mod set_status;
pub mod show;
pub mod stats;
pub mod sync;
pub mod tag;
pub mod verify;

use serde_norway::Value;
use time::{format_description::FormatItem, macros::format_description, OffsetDateTime};

use crate::error::{usage, Result};
use crate::frontmatter::Frontmatter;
use crate::project::Project;
use crate::Ctx;

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

/// Parse repeatable `--field key=value` list filters into `(key, value)` pairs.
pub fn parse_field_filters(args: &[String]) -> Result<Vec<(String, String)>> {
    args.iter()
        .map(|a| {
            a.split_once('=')
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .ok_or_else(|| usage(format!("--field expects key=value, got {a:?}")))
        })
        .collect()
}

/// Bare scalar string form of a YAML value, or `None` if it is composite.
/// Used to compare custom-field values against string filters predictably
/// across string/int/bool/enum types (and list elements).
fn scalar_str(v: &Value) -> Option<String> {
    match v {
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// Whether frontmatter `fm` satisfies every `(key, value)` filter (AND). A
/// filter matches when the field is a scalar equal to the value, or a sequence
/// containing an element equal to it; a missing or composite field never matches.
pub fn field_matches(fm: &Frontmatter, filters: &[(String, String)]) -> bool {
    filters.iter().all(|(key, want)| match fm.get(key) {
        Some(Value::Sequence(seq)) => seq.iter().filter_map(scalar_str).any(|s| &s == want),
        Some(v) => scalar_str(v).as_ref() == Some(want),
        None => false,
    })
}

/// Reconcile references, linkify bodies, and relocate docs to their canonical
/// layout paths after a mutating command, unless `--no-sync`. Best-effort: a
/// parse error elsewhere is reported but does not fail the mutation that already
/// succeeded.
pub fn maybe_sync(ctx: &Ctx, prj: &Project) {
    if ctx.no_sync {
        return;
    }
    if sync::run(prj).is_err() {
        eprintln!("note: skipped sync (run `opys verify` to find the problem)");
    }
}
