//! `opys next` — print the pick oracle's answer as a table, or as the
//! documented JSON a dispatcher parses.
//!
//! A **read**: it opens the project, loads the corpus, and prints. It never
//! syncs, never flushes and never writes, so `--no-sync` means nothing here and
//! the exit code is `0` for every answer, including an empty one. The selection
//! itself is [`crate::next::pick`], which the node's endpoint calls with the
//! same query and serializes the same way — the two surfaces cannot drift
//! because there is only one of them.

use crate::commands::print_lines;
use crate::doc::Doc;
use crate::error::{usage, OpysError, Result};
use crate::next::{self, PickQuery};
use crate::observe;
use crate::Ctx;

use super::stats;

/// `explain` mirrors clap's `Option<Option<String>>`: absent, `--explain`
/// (explain every listed item), or `--explain <ID>` (explain that one).
pub fn run(ctx: &Ctx, q: &PickQuery, json: bool, explain: Option<Option<String>>) -> Result<()> {
    let prj = ctx.open()?;
    // Before the load, so a misspelled status never takes the inventory lock.
    q.validate(&prj.pcfg).map_err(usage)?;
    let (mut store, parse_errors) = ctx.load(&prj)?;
    // On stderr, so `--json` keeps stdout parseable. A document that will not
    // parse is a `verify` job; the oracle answers over the rest, as `query` does.
    for e in &parse_errors {
        eprintln!("warning: {e}");
    }
    let docs: Vec<Doc> = store.all_docs()?.into_iter().map(|(_, d)| d).collect();
    let now = observe::now();
    let mut result = next::pick(&prj, &docs, q, now);

    // The per-item lines only exist for a human: with `--json` the item already
    // carries `priority`, `effective_priority`, `blocks` and `position`, so
    // `--explain` alone adds nothing there. A named id does add the `explain`
    // key, in both renderings.
    let mut lines: Vec<String> = Vec::new();
    match &explain {
        None => {}
        Some(None) => {
            lines = result
                .items
                .iter()
                .map(|i| next::listed_line(i, result.total))
                .collect();
        }
        Some(Some(id)) => {
            let e = next::explain(&prj, &docs, q, id, now)?;
            lines.push(e.message.clone());
            result.explain = Some(e);
        }
    }

    if json {
        let text = serde_json::to_string_pretty(&result)
            .map_err(|e| OpysError::Store(format!("serializing the pick failed: {e}")))?;
        return print_lines([text]);
    }

    let labels = strings(&[
        "#", "ID", "TYPE", "STATUS", "PRIORITY", "BLOCKS", "TITLE", "NOTES",
    ]);
    let rows: Vec<Vec<String>> = result
        .items
        .iter()
        .map(|i| {
            vec![
                i.position.to_string(),
                i.id.clone(),
                i.type_name.clone().unwrap_or_default(),
                i.status.clone(),
                priority_cell(i.priority, i.effective_priority),
                i.blocks.to_string(),
                i.title.clone(),
                i.observations
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            ]
        })
        .collect();
    let mut out = stats::table_body(&labels, &rows);
    if !result.orphaned.is_empty() {
        let olabels = strings(&["ID", "TYPE", "STATUS", "TITLE", "REASON"]);
        let orows: Vec<Vec<String>> = result
            .orphaned
            .iter()
            .map(|o| {
                vec![
                    o.id.clone(),
                    o.type_name.clone().unwrap_or_default(),
                    o.status.clone(),
                    o.title.clone(),
                    o.reason.clone(),
                ]
            })
            .collect();
        out.push('\n');
        out.push_str(&stats::render_table("Orphaned", &olabels, &orows));
    }
    stats::print_markdown(&out, false)?;
    // Plain lines, not markdown: a terminal renderer would reflow them into a
    // paragraph, and each one is a record a caller may want to copy whole.
    if !lines.is_empty() {
        print_lines(lines)?;
    }
    Ok(())
}

fn strings(labels: &[&str]) -> Vec<String> {
    labels.iter().map(|s| (*s).to_string()).collect()
}

/// The `PRIORITY` cell: the number that ordered the row, and — when it was
/// inherited from something the document blocks — the document's own beside it.
fn priority_cell(own: Option<i64>, effective: Option<i64>) -> String {
    match (own, effective) {
        (Some(own), Some(eff)) if own != eff => format!("{eff} (own {own})"),
        (None, Some(eff)) => format!("{eff} (inherited)"),
        (Some(own), _) => own.to_string(),
        (None, None) => "-".to_string(),
    }
}
