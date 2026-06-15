use std::io::Write;

use crate::error::Result;
use crate::Ctx;

pub fn run(ctx: &Ctx, id: &str) -> Result<()> {
    let prj = ctx.open()?;
    let (docs, _) = prj.load_docs();
    let d = prj.find(&docs, id)?;
    let text = std::fs::read_to_string(&d.path)?;
    // Print verbatim, without forcing a trailing newline.
    print!("{text}");
    std::io::stdout().flush()?;
    Ok(())
}
