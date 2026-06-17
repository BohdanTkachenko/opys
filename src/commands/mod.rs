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
use time::{
    format_description::well_known::Rfc3339, format_description::FormatItem,
    macros::format_description, OffsetDateTime,
};

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

/// The current instant as an RFC3339 datetime, second precision (local time,
/// falling back to UTC). Honors the `OPYS_NOW` environment override so tests can
/// pin a deterministic timestamp; the override is returned verbatim.
pub fn now_rfc3339() -> String {
    if let Ok(s) = std::env::var("OPYS_NOW") {
        if !s.is_empty() {
            return s;
        }
    }
    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    let now = now.replace_nanosecond(0).unwrap_or(now);
    now.format(&Rfc3339).expect("RFC3339 formatting")
}

/// Stamp the auto-maintained timestamp fields on a user-initiated write: always
/// refresh `updated`, and set `created` once (only if absent). Reconcile/linkify
/// housekeeping must NOT call this — only meaningful user edits bump `updated`.
pub fn touch(fm: &mut Frontmatter) {
    let now = now_rfc3339();
    if !fm.contains_key("created") {
        fm.set_str("created", now.clone());
    }
    fm.set_str("updated", now);
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

/// Like [`maybe_sync`] but returns the result instead of printing — for callers
/// (e.g. the TUI) that must not write to stdout/stderr and want to surface a
/// sync failure themselves. Returns the number of documents synced (0 when
/// `--no-sync`).
pub fn sync_quiet(ctx: &Ctx, prj: &Project) -> Result<usize> {
    if ctx.no_sync {
        return Ok(0);
    }
    sync::run(prj)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::{format_description::well_known::Rfc3339, OffsetDateTime};

    #[test]
    fn now_rfc3339_is_parseable() {
        let s = now_rfc3339();
        assert!(OffsetDateTime::parse(&s, &Rfc3339).is_ok(), "got {s:?}");
    }

    #[test]
    fn touch_sets_created_when_absent_equal_to_updated() {
        let mut fm = Frontmatter::new();
        touch(&mut fm);
        let created = fm.get_str("created").map(str::to_string);
        let updated = fm.get_str("updated").map(str::to_string);
        assert!(created.is_some());
        assert_eq!(created, updated);
    }

    #[test]
    fn touch_preserves_existing_created_but_refreshes_updated() {
        let mut fm = Frontmatter::new();
        fm.set_str("created", "2020-01-01T00:00:00Z");
        touch(&mut fm);
        assert_eq!(fm.get_str("created"), Some("2020-01-01T00:00:00Z"));
        assert!(fm.get_str("updated").is_some());
        assert_ne!(fm.get_str("updated"), Some("2020-01-01T00:00:00Z"));
    }
}
