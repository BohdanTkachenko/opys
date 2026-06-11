use std::io::Write;

use crate::error::Result;
use crate::Ctx;

pub fn run(ctx: &Ctx, id: &str) -> Result<()> {
    let prj = ctx.open()?;
    let (feats, _) = prj.load();
    let f = prj.find(&feats, id)?;
    let text = std::fs::read_to_string(&f.path)?;
    // Print verbatim, without forcing a trailing newline.
    print!("{text}");
    std::io::stdout().flush()?;
    Ok(())
}
