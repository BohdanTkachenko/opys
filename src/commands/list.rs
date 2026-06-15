use crate::cli::ListFormat;
use crate::commands::{field_matches, parse_field_filters};
use crate::doc::Doc;
use crate::error::Result;
use crate::Ctx;

fn matches(d: &Doc, tag: Option<&str>, status: Option<&str>, fields: &[(String, String)]) -> bool {
    if let Some(tag) = tag {
        let has = d
            .frontmatter
            .tags()
            .map(|ts| ts.iter().any(|t| t == tag))
            .unwrap_or(false);
        if !has {
            return false;
        }
    }
    if let Some(status) = status {
        if d.status() != Some(status) {
            return false;
        }
    }
    field_matches(&d.frontmatter, fields)
}

pub fn run(
    ctx: &Ctx,
    type_name: Option<&str>,
    tag: Option<&str>,
    status: Option<&str>,
    field: &[String],
    format: ListFormat,
) -> Result<()> {
    let prj = ctx.open()?;
    let filters = parse_field_filters(field)?;
    let (docs, _) = prj.load_docs();
    let of_type = |d: &Doc| match type_name {
        None => true,
        Some(tn) => d.id().and_then(|id| prj.pcfg.type_name_for_id(id)) == Some(tn),
    };
    for d in docs
        .iter()
        .filter(|d| of_type(d) && matches(d, tag, status, &filters))
    {
        match format {
            ListFormat::Ids => println!("{}", d.id().unwrap_or("")),
            ListFormat::Paths => println!("{}", d.path.display()),
            ListFormat::Table => {
                let tags = d.frontmatter.tags().unwrap_or_default().join(", ");
                println!(
                    "{}  [{}]  ({})  {}",
                    d.id().unwrap_or(""),
                    d.status().unwrap_or(""),
                    tags,
                    d.title
                );
            }
        }
    }
    Ok(())
}
