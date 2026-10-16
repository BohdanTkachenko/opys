//! `opys log` — append a dated entry to a document's `log` section.
//!
//! The one body edit every agent makes on every work item, made a command: it
//! finds the section the document's type declares with `kind = "log"`, appends
//! `- <UTC timestamp> <text>` to it, bumps `updated`, and runs the ordinary
//! sync pass. The splice is [`body::apply_section_edits`], whose only owner
//! this is ([ADR-0051]), so nothing outside the section moves by a byte.
//!
//! Unlike every other mutating command this one takes a **single** id: the
//! second positional is the entry text, and `-` there already means "read the
//! text from stdin", so an id list would be ambiguous about which of the two
//! the pipe is feeding. `show` is the precedent.
//!
//! It deliberately does not run [`crate::rules`]. A log entry is the thing a
//! `require_log_since_status` rule asks *for*: refusing to record one because
//! the document currently fails that rule (or any unrelated one) would make the
//! checkpoint unrecordable. `verify` still has the last word over the corpus.
//!
//! [ADR-0051]: the `inventory/ADR-0051.md` decision, "SQL is read-only; every
//! write is a lifecycle command".

use crate::body;
use crate::commands::{maybe_sync, now_utc_seconds, print_lines, touch};
use crate::error::{usage, Result};
use crate::project::Project;
use crate::project_config::{SectionKind, SectionSpec};
use crate::store::Store;
use crate::Ctx;

/// Append one entry to a document's log section. Returns the line the command
/// prints. Does not flush/print/sync.
pub fn core(
    prj: &Project,
    store: &mut Store,
    id: &str,
    text: &str,
    section: Option<&str>,
) -> Result<String> {
    let pcfg = &prj.pcfg;
    let tname = pcfg
        .type_name_for_id(id)
        .ok_or_else(|| usage(format!("unrecognized id prefix in {id}")))?
        .to_string();
    let t = &pcfg.types[&tname];
    let sec = resolve_section(&tname, &t.sections, section)?;

    let entry = render_entry(&now_utc_seconds(), text)?;

    let dkey = store.dkey_of(id)?;
    let mut doc = store.doc(dkey)?;

    let spans = body::section_spans(&doc.body);
    let created = match spans.iter().position(|(h, _)| h == &sec.heading) {
        Some(i) => {
            let current = doc.body[spans[i].1.clone()].trim();
            let new_text = if current.is_empty() {
                entry
            } else {
                format!("{current}\n{entry}")
            };
            doc.body = body::apply_section_edits(&doc.body, &spans, &[(i, new_text)]);
            false
        }
        // Declared for the type but absent from this document: scaffold it at
        // the end rather than refusing. A missing section is `verify`'s to
        // report (and `new`'s to have scaffolded); losing the entry over it
        // would be the wrong trade — the text is the thing the user has and the
        // document does not.
        None => {
            let mut b = doc.body.trim_end_matches('\n').to_string();
            b.push_str(&format!("\n\n## {}\n{entry}\n", sec.heading));
            doc.body = b;
            true
        }
    };
    touch(&mut doc.frontmatter);
    store.put_doc(pcfg, Some(dkey), &doc)?;

    Ok(if created {
        format!("{id} logged to '## {}' (section created)", sec.heading)
    } else {
        format!("{id} logged to '## {}'", sec.heading)
    })
}

/// Which section the entry goes to: the type's one `log` section, or the one
/// `--section` names. Every failure here is a usage error (exit 2) naming what
/// the type actually offers — there is nowhere sensible to put the text.
fn resolve_section<'a>(
    tname: &str,
    sections: &'a [SectionSpec],
    want: Option<&str>,
) -> Result<&'a SectionSpec> {
    let logs: Vec<&SectionSpec> = sections
        .iter()
        .filter(|s| s.kind == SectionKind::Log)
        .collect();
    let headings = |v: &[&SectionSpec]| {
        v.iter()
            .map(|s| s.heading.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match want {
        Some(h) => logs
            .iter()
            .find(|s| s.heading == h)
            .copied()
            .ok_or_else(|| {
                if logs.is_empty() {
                    usage(format!("type '{tname}' declares no log section"))
                } else {
                    usage(format!(
                        "'{h}' is not a log section of type '{tname}' (it has: {})",
                        headings(&logs)
                    ))
                }
            }),
        None => match logs.len() {
            0 => Err(usage(format!(
                "type '{tname}' declares no log section — add one with \
                 kind = \"log\" to [[types.{tname}.sections]] in opys.toml"
            ))),
            1 => Ok(logs[0]),
            _ => Err(usage(format!(
                "type '{tname}' has {} log sections ({}) — pass --section",
                logs.len(),
                headings(&logs)
            ))),
        },
    }
}

/// Render one bullet: the stamp, then the first line, then each continuation
/// line indented two spaces under it. Trailing whitespace is trimmed off every
/// line (a blank continuation stays genuinely blank rather than becoming two
/// stray spaces), and leading/trailing blank lines are dropped.
fn render_entry(ts: &str, text: &str) -> Result<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .skip_while(|l| l.is_empty())
        .collect();
    let lines = match lines.iter().rposition(|l| !l.is_empty()) {
        Some(last) => &lines[..=last],
        None => return Err(usage("a log entry needs some text")),
    };
    // The first line joins the bullet after the stamp, so its own leading
    // whitespace would show up as a double space; continuation lines keep
    // theirs beyond the two-space indent, where it may be deliberate.
    let mut out = format!("- {ts} {}", lines[0].trim_start());
    for line in &lines[1..] {
        out.push('\n');
        if !line.is_empty() {
            out.push_str("  ");
            out.push_str(line);
        }
    }
    Ok(out)
}

pub fn run(ctx: &Ctx, id: &str, text: &str, section: Option<&str>) -> Result<()> {
    let prj = ctx.open()?;
    // `-` is the *text*, not an id list: see the module docs. The match is
    // exact, so a real entry that merely starts or ends with a dash (` - `,
    // `- see below`) is text, not a request to read stdin.
    let text = if text == "-" {
        super::read_stdin()?
    } else {
        text.to_string()
    };
    let (mut store, _) = ctx.load(&prj)?;
    let line = core(&prj, &mut store, id, &text, section)?;
    ctx.flush(&prj, store)?;
    print_lines([line])?;
    maybe_sync(ctx, &prj);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(heading: &str, kind: SectionKind) -> SectionSpec {
        toml::from_str(&format!(
            "heading = {heading:?}\nkind = {:?}\n",
            match kind {
                SectionKind::Log => "log",
                SectionKind::Prose => "prose",
                SectionKind::Checklist => "checklist",
                SectionKind::Structured => "structured",
            }
        ))
        .unwrap()
    }

    #[test]
    fn resolve_section_needs_exactly_one_log_section_or_a_name() {
        let none = [spec("Tasks", SectionKind::Checklist)];
        assert!(resolve_section("task", &none, None).is_err());

        let one = [
            spec("Tasks", SectionKind::Checklist),
            spec("Progress", SectionKind::Log),
        ];
        assert_eq!(
            resolve_section("task", &one, None).unwrap().heading,
            "Progress"
        );
        // A `--section` naming the checklist is still not a log section.
        assert!(resolve_section("task", &one, Some("Tasks")).is_err());

        let two = [
            spec("Progress", SectionKind::Log),
            spec("Decisions", SectionKind::Log),
        ];
        let err = resolve_section("task", &two, None).unwrap_err().to_string();
        assert!(err.contains("Progress, Decisions"), "{err}");
        assert_eq!(
            resolve_section("task", &two, Some("Decisions"))
                .unwrap()
                .heading,
            "Decisions"
        );
    }

    #[test]
    fn render_entry_indents_continuations_and_rejects_empty_text() {
        assert_eq!(
            render_entry("2026-01-02T03:04:05Z", "one").unwrap(),
            "- 2026-01-02T03:04:05Z one"
        );
        assert_eq!(
            render_entry("2026-01-02T03:04:05Z", "\nfirst  \nsecond\n\nthird\n\n").unwrap(),
            "- 2026-01-02T03:04:05Z first\n  second\n\n  third"
        );
        assert!(render_entry("2026-01-02T03:04:05Z", "  \n\n").is_err());
    }
}
