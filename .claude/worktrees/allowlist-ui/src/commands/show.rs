//! `opys show` — print a document's file verbatim (contractually the on-disk
//! bytes, not the canonical reconstruction). The path is resolved through the
//! corpus store; `--refs` appends code mentions of the id per `[file_refs]`.

use std::io::Write;

use crate::error::Result;
use crate::file_refs;
use crate::store::{g_str, IntoParam};
use crate::Ctx;

pub fn run(ctx: &Ctx, id: &str, refs: bool) -> Result<()> {
    let prj = ctx.open()?;
    let (mut store, _) = ctx.load(&prj)?;
    let dkey = store.dkey_of(id)?;
    let relpath = store
        .scalar(
            "SELECT path FROM docs WHERE dkey = $1",
            vec![dkey.into_param()],
        )?
        .as_ref()
        .and_then(g_str)
        .unwrap_or_default();
    let path = prj.base.join(relpath);
    let text = std::fs::read_to_string(&path)?;
    // Print verbatim, without forcing a trailing newline.
    print!("{text}");
    std::io::stdout().flush()?;

    if refs {
        if !text.ends_with('\n') {
            println!();
        }
        let hits = file_refs::scan(&prj, &[id]);
        println!("\n--- file references ---");
        if hits.is_empty() {
            println!("(none)");
        } else {
            for h in &hits {
                println!("{}:{}: {}", h.path.display(), h.line, h.text);
            }
        }
    }
    Ok(())
}
