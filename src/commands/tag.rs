use crate::commands::{maybe_sync, split_csv};
use crate::error::{usage, Result};
use crate::project;
use crate::Ctx;

pub fn run(ctx: &Ctx, id: &str, add: Option<&str>, remove: Option<&str>) -> Result<()> {
    let prj = ctx.open()?;
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

    project::save_doc(&prj, d)?;
    println!("{id} tags: {}", tags.join(", "));
    maybe_sync(ctx, &prj);
    Ok(())
}
