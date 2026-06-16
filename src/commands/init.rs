use crate::error::Result;
use crate::project::resolve_base;
use crate::project_config::DEFAULT_DOC_DIR;
use crate::templates::{CLAUDE_MD_SNIPPET, DEFAULT_OPYS_CONFIG};
use crate::Ctx;

pub fn run(ctx: &Ctx) -> Result<()> {
    let base = resolve_base(&ctx.root, &ctx.dir);
    std::fs::create_dir_all(&base)?;

    let cfg = base.join("opys.toml");
    if cfg.exists() {
        println!("{} already exists; leaving it untouched", cfg.display());
    } else {
        std::fs::write(&cfg, DEFAULT_OPYS_CONFIG)?;
        println!(
            "created {} — edit it to model your document types",
            cfg.display()
        );
    }

    std::fs::create_dir_all(base.join(DEFAULT_DOC_DIR))?;
    std::fs::create_dir_all(base.join("runbooks"))?;

    println!("\nAdd this to your CLAUDE.md / agent instructions:\n");
    println!("{CLAUDE_MD_SNIPPET}");
    Ok(())
}
