use crate::commands::{maybe_sync, split_csv};
use crate::error::Result;
use crate::project;
use crate::Ctx;

pub fn run(ctx: &Ctx, id: &str, add: Option<&str>, remove: Option<&str>) -> Result<()> {
    let prj = ctx.open()?;
    prj.require_wi_cfg()?;
    let (mut items, _) = prj.load_work_items();
    let w = prj.find_wi_mut(&mut items, id)?;

    let mut tags = w.frontmatter.tags().unwrap_or_default();
    for t in split_csv(add.unwrap_or("")) {
        if !tags.contains(&t) {
            tags.push(t);
        }
    }
    for t in split_csv(remove.unwrap_or("")) {
        tags.retain(|x| x != &t);
    }

    // Work-item tags are optional, so they may drop to empty.
    if tags.is_empty() {
        w.frontmatter.remove("tags");
    } else {
        w.frontmatter.set_tags(&tags);
    }
    project::write_work_item(w)?;
    println!("{id} tags: {}", tags.join(", "));
    maybe_sync(ctx, &prj);
    Ok(())
}
