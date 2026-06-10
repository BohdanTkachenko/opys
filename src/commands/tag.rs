use crate::commands::split_csv;
use crate::error::{usage, Result};
use crate::project::{self, Project};

pub fn run(root: &str, id: &str, add: Option<&str>, remove: Option<&str>) -> Result<()> {
    let prj = Project::open(root)?;
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
    Ok(())
}
