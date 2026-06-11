use crate::commands::{maybe_sync, split_csv};
use crate::error::{usage, Result};
use crate::feature::Feature;
use crate::frontmatter::Frontmatter;
use crate::project::{self, Project};
use crate::Ctx;

pub fn run(ctx: &Ctx, title: &str, tags: &str, status: &str, fields: &[String]) -> Result<()> {
    let prj = Project::open(&ctx.root, &ctx.dir)?;
    let (feats, _) = prj.load();
    let id = prj.next_id(&feats);

    let tags = split_csv(tags);
    if tags.is_empty() {
        return Err(usage("at least one tag is required (--tags a,b)"));
    }

    let mut fm = Frontmatter::new();
    fm.set_str("id", &id);
    fm.set_str("status", status);
    fm.set_tags(&tags);
    for kv in fields {
        let (k, v) = project::parse_field(kv)?;
        fm.insert(&k, v);
    }

    let body = format!("# {title}\n");
    let path = prj.path_for(&id);
    let feature = Feature {
        path: path.clone(),
        frontmatter: fm,
        body,
        title: title.to_string(),
    };
    std::fs::write(&path, feature.to_text())?;
    println!("{}", path.display());
    maybe_sync(ctx, &prj);
    Ok(())
}
