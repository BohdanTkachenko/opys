use crate::commands::{maybe_sync, split_csv};
use crate::error::{usage, Result};
use crate::project;
use crate::Ctx;

pub fn run(ctx: &Ctx, id: &str, add: Option<&str>, remove: Option<&str>) -> Result<()> {
    let prj = ctx.open()?;
    let (mut feats, _) = prj.load();
    let f = prj.find_mut(&mut feats, id)?;

    let mut tags = f.frontmatter.tags().unwrap_or_default();
    for t in split_csv(add.unwrap_or("")) {
        if !tags.contains(&t) {
            tags.push(t);
        }
    }
    for t in split_csv(remove.unwrap_or("")) {
        tags.retain(|x| x != &t);
    }
    if tags.is_empty() {
        return Err(usage("a feature must keep at least one tag"));
    }

    f.frontmatter.set_tags(&tags);
    project::write_feature(f)?;
    println!("{id} tags: {}", tags.join(", "));
    maybe_sync(ctx, &prj);
    Ok(())
}
