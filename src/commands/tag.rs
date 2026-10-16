//! `opys tag` — add/remove tags on documents through the corpus store.

use crate::commands::{apply_batch, maybe_sync, print_lines, resolve_ids, split_csv, touch};
use crate::error::{usage, Result};
use crate::project::Project;
use crate::store::Store;
use crate::Ctx;

/// Add and/or remove tags on a document; returns the doc's resulting tag list.
/// Does not flush/print/sync.
pub fn core(
    prj: &Project,
    store: &mut Store,
    id: &str,
    add: Option<&str>,
    remove: Option<&str>,
) -> Result<Vec<String>> {
    let tags_required = prj
        .pcfg
        .type_name_for_id(id)
        .map(|n| prj.pcfg.types[n].tags_required)
        .unwrap_or(false);

    let dkey = store.dkey_of(id)?;
    let mut doc = store.doc(dkey)?;

    let mut tags = doc.frontmatter.tags().unwrap_or_default();
    for t in split_csv(add.unwrap_or("")) {
        if !tags.contains(&t) {
            tags.push(t);
        }
    }
    for t in split_csv(remove.unwrap_or("")) {
        tags.retain(|x| x != &t);
    }
    if tags.is_empty() {
        if tags_required {
            return Err(usage("this type requires at least one tag"));
        }
        doc.frontmatter.remove("tags");
    } else {
        doc.frontmatter.set_tags(&tags);
    }
    touch(&mut doc.frontmatter);

    store.put_doc(&prj.pcfg, Some(dkey), &doc)?;
    Ok(tags)
}

pub fn run(ctx: &Ctx, ids: &[String], add: Option<&str>, remove: Option<&str>) -> Result<()> {
    let prj = ctx.open()?;
    let ids = resolve_ids(ids)?;
    let (mut store, _) = ctx.load(&prj)?;
    let lines = apply_batch(&ids, |id| {
        let tags = core(&prj, &mut store, id, add, remove)?;
        Ok(format!("{id} tags: {}", tags.join(", ")))
    })?;
    ctx.flush(&prj, store)?;
    print_lines(lines)?;
    maybe_sync(ctx, &prj);
    Ok(())
}
