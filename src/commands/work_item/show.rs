use std::io::Write;

use crate::error::Result;
use crate::Ctx;

pub fn run(ctx: &Ctx, id: &str) -> Result<()> {
    let prj = ctx.open()?;
    prj.require_wi_cfg()?;
    let (items, _) = prj.load_work_items();
    let w = prj.find_wi(&items, id)?;
    let text = std::fs::read_to_string(&w.path)?;
    print!("{text}");
    std::io::stdout().flush()?;
    Ok(())
}
