use std::path::Path;

use crate::cli::SchemaKind;
use crate::error::Result;
use crate::schema;
use crate::Ctx;

pub fn run(ctx: &Ctx, kind: SchemaKind, out: Option<&str>) -> Result<()> {
    let value = match kind {
        SchemaKind::Config => schema::config_schema(),
        SchemaKind::Frontmatter => {
            let prj = ctx.open()?;
            schema::frontmatter_schema(&prj.cfg)
        }
    };
    let text = serde_json::to_string_pretty(&value).expect("schema serializes");
    match out {
        Some(path) => {
            if let Some(parent) = Path::new(path).parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            std::fs::write(path, text + "\n")?;
            println!("{path}");
        }
        None => println!("{text}"),
    }
    Ok(())
}
