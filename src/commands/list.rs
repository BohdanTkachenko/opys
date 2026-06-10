use crate::cli::ListFormat;
use crate::error::Result;
use crate::feature::Feature;
use crate::project::Project;

fn matches(f: &Feature, tag: Option<&str>, status: Option<&str>) -> bool {
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
    true
}

pub fn run(root: &str, tag: Option<&str>, status: Option<&str>, format: ListFormat) -> Result<()> {
    let prj = Project::open(root)?;
    let (feats, _) = prj.load();
    for f in feats.iter().filter(|f| matches(f, tag, status)) {
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
