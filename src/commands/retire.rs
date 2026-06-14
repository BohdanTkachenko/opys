use crate::commands::{maybe_sync, today};
use crate::error::Result;
use crate::project;
use crate::Ctx;

pub fn run(ctx: &Ctx, id: &str, reason: &str) -> Result<()> {
    let prj = ctx.open()?;
    let (feats, _) = prj.load();
    let f = prj.find(&feats, id)?;
    let path = f.path.clone();

    let rp = prj.fdir.join("_retired.txt");
    let line = format!("{id}  # retired {}: {reason}", today());
    project::write_id_ledger_entry(&rp, id, &line)?;

    std::fs::remove_file(&path)?;
    println!("retired {id} (ID will never be reused)");
    maybe_sync(ctx, &prj);
    Ok(())
}
