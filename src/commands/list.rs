use crate::cli::ListFormat;
use crate::commands::{field_matches, parse_field_filters};
use crate::error::Result;
use crate::feature::Feature;
use crate::Ctx;

fn matches(
    f: &Feature,
    tag: Option<&str>,
    status: Option<&str>,
    fields: &[(String, String)],
) -> bool {
    if let Some(tag) = tag {
        let has = f
            .frontmatter
            .tags()
            .map(|ts| ts.iter().any(|t| t == tag))
            .unwrap_or(false);
        if !has {
            return false;
        }
    }
    if let Some(status) = status {
        if f.status() != Some(status) {
            return false;
        }
    }
    field_matches(&f.frontmatter, fields)
}

pub fn run(
    ctx: &Ctx,
    tag: Option<&str>,
    status: Option<&str>,
    field: &[String],
    format: ListFormat,
) -> Result<()> {
    let prj = ctx.open()?;
    let filters = parse_field_filters(field)?;
    let (feats, _) = prj.load();
    for f in feats.iter().filter(|f| matches(f, tag, status, &filters)) {
        match format {
            ListFormat::Ids => println!("{}", f.id().unwrap_or("")),
            ListFormat::Paths => println!("{}", f.path.display()),
            ListFormat::Table => {
                let tags = f.frontmatter.tags().unwrap_or_default().join(", ");
                println!(
                    "{}  [{}]  ({})  {}",
                    f.id().unwrap_or(""),
                    f.status().unwrap_or(""),
                    tags,
                    f.title
                );
            }
        }
    }
    Ok(())
}
