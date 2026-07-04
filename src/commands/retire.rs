//! `opys retire` — delete a document and reserve its id forever (ledger).

use crate::commands::{expand_ids, for_each_id, maybe_sync};
use crate::error::Result;
use crate::project::Project;
use crate::store::Store;
use crate::Ctx;

/// Reserve `id` (with its last title) in the retired ledger and drop it from the
/// store; flush deletes the file and writes the ledger. Does not print/sync.
fn retire_one(prj: &Project, store: &mut Store, id: &str) -> Result<()> {
    let dkey = store.dkey_of(id)?;
    let title = store.title_of(id)?.unwrap_or_default();
    store.retire_id(id, &title)?;
    store.delete_doc(dkey)?;
    let _ = prj; // (kept for signature symmetry with the other cores)
    Ok(())
}

pub fn run(ctx: &Ctx, ids: &str, reason: &str) -> Result<()> {
    let prj = ctx.open()?;
    let ids = expand_ids(ids)?;
    let (mut store, _) = ctx.load(&prj)?;
    let res = for_each_id(&ids, |id| {
        retire_one(&prj, &mut store, id)?;
        if reason.is_empty() {
            println!("retired {id} (ID will never be reused)");
        } else {
            println!("retired {id}: {reason} (ID will never be reused)");
        }
        Ok(())
    });
    ctx.flush(&prj, store)?;
    maybe_sync(ctx, &prj);
    res
}
