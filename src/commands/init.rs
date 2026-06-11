use crate::error::Result;
use crate::project::resolve_base;
use crate::templates::{CLAUDE_MD_SNIPPET, DEFAULT_CONFIG};
use crate::Ctx;

pub fn run(ctx: &Ctx) -> Result<()> {
    let base = resolve_base(&ctx.root, &ctx.dir);
    let fdir = base.join("features");
    std::fs::create_dir_all(&fdir)?;

    let cfg = fdir.join("_config.toml");
    if cfg.exists() {
        println!("{} already exists; leaving it untouched", cfg.display());
    } else {
        std::fs::write(&cfg, DEFAULT_CONFIG)?;
        println!(
            "created {} — edit prefix and custom fields to taste",
            cfg.display()
        );
    }

    std::fs::create_dir_all(base.join("runbooks"))?;

    println!("\nAdd this to your CLAUDE.md / agent instructions:\n");
    println!("{CLAUDE_MD_SNIPPET}");
    Ok(())
}
