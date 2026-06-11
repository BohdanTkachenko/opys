use std::io::Write;

use crate::commands::{maybe_sync, today};
use crate::error::Result;
use crate::Ctx;

pub fn run(ctx: &Ctx, id: &str, reason: &str) -> Result<()> {
    let prj = ctx.open()?;
    let (feats, _) = prj.load();
    let f = prj.find(&feats, id)?;
    let path = f.path.clone();

    let rp = prj.fdir.join("_retired.txt");
    let mut fh = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&rp)?;
    writeln!(fh, "{id}  # retired {}: {reason}", today())?;

    std::fs::remove_file(&path)?;
    println!("retired {id} (ID will never be reused)");
    maybe_sync(ctx, &prj);
    Ok(())
}
