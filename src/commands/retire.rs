//! `opys retire` — delete a document and reserve its id forever (ledger).

use crate::commands::{expand_ids, for_each_id, maybe_sync, today};
use crate::error::Result;
use crate::project::Project;
use crate::store::{retire_line, Store};
use crate::Ctx;

/// Log `id` to the retired ledger and drop it from the store; flush deletes the
/// file and rewrites the ledger. Does not print/sync.
fn retire_one(prj: &Project, store: &mut Store, id: &str, reason: &str) -> Result<()> {
    let dkey = store.dkey_of(id)?;
    store.retire_id(id, &retire_line(id, &today(), reason))?;
    store.delete_doc(dkey)?;
    let _ = prj; // (kept for signature symmetry with the other cores)
    Ok(())
}

pub fn run(ctx: &Ctx, ids: &str, reason: &str) -> Result<()> {
    let prj = ctx.open()?;
    let ids = expand_ids(ids)?;
    let (mut store, _) = Store::open(&prj)?;
    let res = for_each_id(&ids, |id| {
        retire_one(&prj, &mut store, id, reason)?;
        println!("retired {id} (ID will never be reused)");
        Ok(())
    });
    store.flush(&prj)?;
    maybe_sync(ctx, &prj);
    res
}
