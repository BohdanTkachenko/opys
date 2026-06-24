use std::io::Write;

use crate::error::Result;
use crate::file_refs;
use crate::Ctx;

pub fn run(ctx: &Ctx, id: &str, refs: bool) -> Result<()> {
    let prj = ctx.open()?;
    let (docs, _) = prj.load_docs();
    let d = prj.find(&docs, id)?;
    let text = std::fs::read_to_string(&d.path)?;
    // Print verbatim, without forcing a trailing newline.
    print!("{text}");
    std::io::stdout().flush()?;

    if refs {
        if !text.ends_with('\n') {
            println!();
        }
        let id = d.id().unwrap_or(id);
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
