use crate::commands::{maybe_sync, today};
use crate::error::Result;
use crate::project;
use crate::Ctx;

pub fn run(ctx: &Ctx, id: &str, reason: &str) -> Result<()> {
    let prj = ctx.open()?;
    let (docs, _) = prj.load_docs();
    let d = prj.find(&docs, id)?;
    let path = d.path.clone();

    let rp = prj.base.join("_retired.txt");
    let line = format!("{id}  # retired {}: {reason}", today());
    project::write_id_ledger_entry(&rp, id, &line)?;

    std::fs::remove_file(&path)?;
    println!("retired {id} (ID will never be reused)");
    maybe_sync(ctx, &prj);
    Ok(())
}
