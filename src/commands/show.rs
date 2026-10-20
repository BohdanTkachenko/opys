//! `opys show` — print a document's file verbatim, or its canonical effective
//! copy when read from git for a type with work branches. The path is resolved
//! through the corpus store; `--refs` appends code mentions per `[file_refs]`.

use crate::commands::{print_lines, print_text};
use crate::error::Result;
use crate::file_refs;
use crate::store::{g_str, IntoParam};
use crate::Ctx;

pub fn run(ctx: &Ctx, id: &str, refs: bool) -> Result<()> {
    let prj = ctx.open()?;
    let (mut store, _) = ctx.load(&prj)?;
    let text = if prj
        .pcfg
        .type_name_for_id(id)
        .is_some_and(|n| !prj.pcfg.types[n].work_branches.is_empty())
    {
        let docs: Vec<_> = store.all_docs()?.into_iter().map(|(_, d)| d).collect();
        let view = crate::work::effective(&prj, &docs, crate::observe::now());
        view.ensure_main()?;
        for notice in &view.notices {
            eprintln!("warning: {notice}");
        }
        let doc = prj.find(&view.docs, id)?;
        if let Some(reference) = view.reference(doc) {
            print_lines([format!("--- work copy from {reference} ---")])?;
            doc.to_text()
        } else {
            print_lines(["--- main copy ---"])?;
            if view.main_is_current {
                std::fs::read_to_string(&doc.path)?
            } else {
                doc.to_text()
            }
        }
    } else {
        let dkey = store.dkey_of(id)?;
        let relpath = store
            .scalar(
                "SELECT path FROM docs WHERE dkey = $1",
                vec![dkey.into_param()],
            )?
            .as_ref()
            .and_then(g_str)
            .unwrap_or_default();
        std::fs::read_to_string(prj.base.join(relpath))?
    };
    // Print verbatim, without forcing a trailing newline.
    print_text(&text)?;

    if refs {
        let mut lines: Vec<String> = Vec::new();
        if !text.ends_with('\n') {
            lines.push(String::new());
        }
        let hits = file_refs::scan(&prj, &[id]);
        lines.push("\n--- file references ---".to_string());
        if hits.is_empty() {
            lines.push("(none)".to_string());
        } else {
            for h in &hits {
                lines.push(format!("{}:{}: {}", h.path.display(), h.line, h.text));
            }
        }
        print_lines(lines)?;
    }
    Ok(())
}
