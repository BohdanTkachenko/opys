use crate::error::Result;
use crate::project::resolve_base;
use crate::templates::{DEFAULT_WI_CONFIG, WI_CLAUDE_MD_SNIPPET};
use crate::Ctx;

pub fn run(ctx: &Ctx) -> Result<()> {
    let base = resolve_base(&ctx.root, &ctx.dir);
    let wdir = base.join("work-items");
    std::fs::create_dir_all(&wdir)?;

    let cfg = wdir.join("_config.toml");
    if cfg.exists() {
        println!("{} already exists; leaving it untouched", cfg.display());
    } else {
        std::fs::write(&cfg, DEFAULT_WI_CONFIG)?;
        println!(
            "created {} — edit pad and custom fields to taste",
            cfg.display()
        );
    }

    println!("\nAdd this to your CLAUDE.md / agent instructions:\n");
    println!("{WI_CLAUDE_MD_SNIPPET}");
    Ok(())
}
