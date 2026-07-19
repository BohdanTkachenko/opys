//! `opys retire` — delete a document and reserve its id forever (ledger).

use crate::commands::{expand_ids, for_each_id, maybe_sync};
use crate::error::Result;
use crate::project::Project;
use crate::store::Store;
use crate::{refs, rules, Ctx};

/// Reserve `id` (with its last title) in the retired ledger, drop it from the
/// store, and strike every inbound reference into a `~~title~~` tombstone
/// (exactly like `close`) so the deletion leaves no live link dangling; flush
/// deletes the file and writes the ledger. Returns a warning per rule problem
/// the strikes leave behind on a referencing doc (a struck link no longer
/// satisfies e.g. `requires_link` — the corpus should not go red silently).
/// Does not print/sync.
fn retire_one(prj: &Project, store: &mut Store, id: &str) -> Result<Vec<String>> {
    let dkey = store.dkey_of(id)?;
    let doc = store.doc(dkey)?;
    let title = doc.title.clone();
    store.retire_id(id, &title)?;
    store.delete_doc(dkey)?;
    let struck = store.strike_inbound(&prj.pcfg, id, &refs::strike(&title))?;
    // Relation entries living only in the retired doc's own maps (e.g. the
    // tombstone of an earlier close) vanish with its file — reserve any id they
    // were the last anchor for.
    let carried: Vec<(String, String)> = refs::RELATION_FIELDS
        .iter()
        .flat_map(|f| refs::parse_in(&doc.frontmatter, f))
        .map(|(tid, val)| {
            let t = refs::unstrike(&val).to_string();
            (tid, t)
        })
        .collect();
    store.reserve_unanchored(&carried)?;

    let doc_ids = store.doc_ids()?;
    let mut warnings = Vec::new();
    for k in struck {
        let d = store.doc(k)?;
        let (Some(did), Some(tname)) = (d.id(), d.id().and_then(|i| prj.pcfg.type_name_for_id(i)))
        else {
            continue;
        };
        for p in rules::evaluate(
            &prj.pcfg,
            tname,
            d.status().unwrap_or(""),
            &d.frontmatter,
            &d.body,
            &doc_ids,
        ) {
            warnings.push(format!("{did}: {p} (after retiring {id})"));
        }
    }
    Ok(warnings)
}

pub fn run(ctx: &Ctx, ids: &str, reason: Option<&str>) -> Result<()> {
    let prj = ctx.open()?;
    let ids = expand_ids(ids)?;
    let (mut store, _) = ctx.load(&prj)?;
    let res = for_each_id(&ids, |id| {
        let warnings = retire_one(&prj, &mut store, id)?;
        match reason {
            Some(r) if !r.is_empty() => {
                println!("retired {id}: {r} (ID will never be reused)")
            }
            _ => println!("retired {id} (ID will never be reused)"),
        }
        for w in warnings {
            eprintln!("warning: {w}");
        }
        Ok(())
    });
    ctx.flush(&prj, store)?;
    maybe_sync(ctx, &prj);
    res
}
