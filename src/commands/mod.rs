//! Subcommand implementations. Each `run` takes the invocation [`Ctx`] plus
//! its parsed arguments.

pub mod agent_rules;
pub mod block;
pub mod cleanup;
pub mod close;
pub mod config;
pub mod edit;
#[cfg(feature = "history")]
pub mod history;
pub mod import;
pub mod init;
pub mod list;
pub mod new;
pub mod query;
pub mod renumber;
pub mod retire;
pub mod set_status;
pub mod show;
pub mod stats;
pub mod sync;
pub mod tag;
pub mod tags;
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

/// The key portion of a tag, for filtering by key: the text before the first
/// `:` or `=`. A bare tag is its own key (`osc` → `osc`), a namespaced or keyed
/// tag exposes its head (`area:parsing` / `priority=high` → `area` / `priority`).
pub fn tag_key(tag: &str) -> &str {
    tag.split([':', '=']).next().unwrap_or(tag)
}

/// Resolve the id arguments of a mutating command into a deduplicated,
/// order-preserving list. Every id-taking command accepts a batch three ways,
/// interchangeably: several positionals (`FEAT-1 FEAT-2`), a comma-separated
/// list (`FEAT-1,FEAT-2`), or the literal `-`, which reads the list from stdin
/// (comma, space and newline separation all accepted) — so
/// `opys list --format ids | opys close -` composes directly. Stdin is read at
/// most once however many `-` arguments appear. Errors if no ids remain after
/// trimming.
///
/// A batch is all-or-nothing: the caller applies the ids in order against one
/// loaded store and flushes only after every one succeeded, so a widened list
/// that hits a bad id writes nothing rather than landing half of it.
pub fn resolve_ids(args: &[String]) -> Result<Vec<String>> {
    let stdin = if args.iter().any(|a| a.trim() == "-") {
        Some(read_stdin()?)
    } else {
        None
    };
    expand_ids(args, stdin.as_deref())
}

/// The pure half of [`resolve_ids`]: expand `args` against an already-read
/// `stdin` text (`None` when no argument asked for it).
fn expand_ids(args: &[String], stdin: Option<&str>) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for arg in args {
        let ids = if arg.trim() == "-" {
            split_ids(stdin.unwrap_or_default())
        } else {
            split_csv(arg)
        };
        for id in ids {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    if out.is_empty() {
        return Err(usage("expected at least one id"));
    }
    Ok(out)
}

/// Split an id list on any mix of comma, space, and newline separation.
fn split_ids(s: &str) -> Vec<String> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// Read stdin to a string, for the `-` id argument.
fn read_stdin() -> Result<String> {
    use std::io::Read;
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    Ok(buf)
}

/// Apply a batch of ids in order against one loaded store, returning the line
/// each success would print.
///
/// All-or-nothing: the first failure aborts the batch (naming the id when there
/// is more than one — a lone id keeps its error verbatim) and the caller returns
/// without flushing, so a partly-applied batch never reaches disk. The output is
/// buffered rather than printed as the loop runs: announcing `FEAT-0001 -> done`
/// for a write that was then abandoned would be a lie.
pub fn apply_batch<F>(ids: &[String], mut f: F) -> Result<Vec<String>>
where
    F: FnMut(&str) -> Result<String>,
{
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        match f(id) {
            Ok(line) => out.push(line),
            Err(e) if ids.len() == 1 => return Err(e),
            Err(e) => {
                return Err(usage(format!(
                    "{id}: {e} — abandoned the batch of {} ids; nothing was written",
                    ids.len()
                )))
            }
        }
    }
    Ok(out)
}

/// Write output to stdout, treating a reader that closed the pipe as a clean
/// stop rather than a crash.
///
/// Rust ignores `SIGPIPE`, so the print macros *panic* ("failed printing to
/// stdout: Broken pipe") the moment the far end of a pipe goes away: an
/// `opys list --format ids | head -1` ends in a Rust backtrace and exit 101
/// where every other Unix producer stops quietly — and that pipe is the
/// sanctioned bulk-edit workflow. So no command prints to stdout with the
/// macros: everything goes through [`print_lines`] / [`print_text`]. (Warnings
/// and problems still go to stderr with `eprintln!`, which the pipe never
/// touches.)
fn to_stdout(
    write: impl FnOnce(&mut std::io::StdoutLock<'_>) -> std::io::Result<()>,
) -> Result<()> {
    use std::io::Write as _;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    match write(&mut out).and_then(|()| out.flush()) {
        Ok(()) => Ok(()),
        // The reader went away (`| head`, `| grep -q`, `| fzf`): stop, quietly.
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Print one output line per item, newline-terminated. See [`to_stdout`] for
/// why this exists instead of `println!`.
pub fn print_lines<I, S>(lines: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    use std::io::Write as _;
    to_stdout(|out| {
        for line in lines {
            writeln!(out, "{}", line.as_ref())?;
        }
        Ok(())
    })
}

/// Print `text` verbatim, adding no trailing newline (`show` reproduces a
/// document byte for byte). See [`to_stdout`].
pub fn print_text(text: &str) -> Result<()> {
    use std::io::Write as _;
    to_stdout(|out| out.write_all(text.as_bytes()))
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
    if sync::run(prj, ctx.backend.as_ref()).is_err() {
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
    sync::run(prj, ctx.backend.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::{format_description::well_known::Rfc3339, OffsetDateTime};

    fn ids(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).to_string()).collect()
    }

    #[test]
    fn expand_ids_takes_several_positionals_and_comma_lists() {
        assert_eq!(
            expand_ids(&ids(&["FEAT-1", "FEAT-2,FEAT-3", " FEAT-4 "]), None).unwrap(),
            ids(&["FEAT-1", "FEAT-2", "FEAT-3", "FEAT-4"])
        );
    }

    #[test]
    fn expand_ids_dedupes_preserving_first_seen_order() {
        assert_eq!(
            expand_ids(&ids(&["FEAT-2", "FEAT-1,FEAT-2", "FEAT-1"]), None).unwrap(),
            ids(&["FEAT-2", "FEAT-1"])
        );
    }

    #[test]
    fn expand_ids_reads_dash_from_stdin_across_whitespace_and_commas() {
        let piped = "FEAT-1\nFEAT-2 FEAT-3\n\n  \nFEAT-4,FEAT-5\n";
        assert_eq!(
            expand_ids(&ids(&["-"]), Some(piped)).unwrap(),
            ids(&["FEAT-1", "FEAT-2", "FEAT-3", "FEAT-4", "FEAT-5"])
        );
    }

    #[test]
    fn expand_ids_mixes_the_stdin_list_with_explicit_ids_once() {
        assert_eq!(
            expand_ids(&ids(&["FEAT-1", "-", "-"]), Some("FEAT-2 FEAT-1")).unwrap(),
            ids(&["FEAT-1", "FEAT-2"])
        );
    }

    #[test]
    fn expand_ids_rejects_an_empty_list() {
        assert!(expand_ids(&ids(&[" ", ","]), None).is_err());
        assert!(expand_ids(&ids(&["-"]), Some("\n  \n")).is_err());
        assert!(expand_ids(&[], None).is_err());
    }

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
