use crate::error::Result;
use crate::project::start_dir;
use crate::project_config::DEFAULT_BASE;
use crate::templates::{CLAUDE_MD_SNIPPET, DEFAULT_OPYS_CONFIG};
use crate::Ctx;

pub fn run(ctx: &Ctx) -> Result<()> {
    let root = start_dir(&ctx.root)?;
    std::fs::create_dir_all(&root)?;

    let cfg = root.join("opys.toml");
    if cfg.exists() {
        println!("{} already exists; leaving it untouched", cfg.display());
    } else {
        std::fs::write(&cfg, DEFAULT_OPYS_CONFIG)?;
        println!(
            "created {} — edit it to model your document types",
            cfg.display()
        );
    }

    // Scaffold the default inventory base (opys/); documents live flat in it.
    std::fs::create_dir_all(root.join(DEFAULT_BASE))?;

    println!("\nAdd this to your CLAUDE.md / agent instructions:\n");
    println!("{CLAUDE_MD_SNIPPET}");
    Ok(())
}
