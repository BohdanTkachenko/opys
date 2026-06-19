use crate::commands::{expand_ids, for_each_id, maybe_sync, today};
use crate::error::Result;
use crate::project::{self, Project};
use crate::Ctx;

/// Delete `id`'s file and log its ID to the retired ledger so it is never
/// reallocated. Does not print or sync.
fn retire_one(prj: &Project, id: &str, reason: &str) -> Result<()> {
    let (docs, _) = prj.load_docs();
    let d = prj.find(&docs, id)?;
    let path = d.path.clone();

    let rp = prj.base.join("_retired.txt");
    let line = format!("{id}  # retired {}: {reason}", today());
    project::write_id_ledger_entry(&rp, id, &line)?;

    std::fs::remove_file(&path)?;
    Ok(())
}

pub fn run(ctx: &Ctx, ids: &str, reason: &str) -> Result<()> {
    let prj = ctx.open()?;
    let ids = expand_ids(ids)?;
    let res = for_each_id(&ids, |id| {
        retire_one(&prj, id, reason)?;
        println!("retired {id} (ID will never be reused)");
        Ok(())
    });
    maybe_sync(ctx, &prj);
    res
}
