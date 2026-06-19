use crate::commands::{expand_ids, for_each_id, maybe_sync, split_csv, touch};
use crate::doc::Doc;
use crate::error::{usage, Result};
use crate::project::{self, Project};
use crate::Ctx;

/// Add and/or remove tags on a document; returns the saved [`Doc`]. Does not
/// print or sync — the shared core for the CLI wrapper and the TUI.
pub fn core(prj: &Project, id: &str, add: Option<&str>, remove: Option<&str>) -> Result<Doc> {
    let tags_required = prj
        .pcfg
        .type_name_for_id(id)
        .map(|n| prj.pcfg.types[n].tags_required)
        .unwrap_or(false);

    let (mut docs, _) = prj.load_docs();
    let d = prj.find_mut(&mut docs, id)?;

    let mut tags = d.frontmatter.tags().unwrap_or_default();
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
        d.frontmatter.remove("tags");
    } else {
        d.frontmatter.set_tags(&tags);
    }
    touch(&mut d.frontmatter);

    project::save_doc(prj, d)?;
    let idx = docs
        .iter()
        .position(|x| x.id() == Some(id))
        .expect("doc just saved is present");
    Ok(docs.swap_remove(idx))
}

pub fn run(ctx: &Ctx, ids: &str, add: Option<&str>, remove: Option<&str>) -> Result<()> {
    let prj = ctx.open()?;
    let ids = expand_ids(ids)?;
    let res = for_each_id(&ids, |id| {
        let doc = core(&prj, id, add, remove)?;
        let tags = doc.frontmatter.tags().unwrap_or_default();
        println!("{id} tags: {}", tags.join(", "));
        Ok(())
    });
    maybe_sync(ctx, &prj);
    res
}
