//! `opys config …` — project-config commands.
//!
//! Currently just `config init`, which scaffolds the opinionated default
//! `opys.toml` for the upcoming universal typed-document engine. Nothing reads
//! that file yet; this only generates it so the config shape can be reviewed and
//! iterated on. Future subcommands (`config validate`, `config show`) belong
//! here too.

use crate::error::Result;
use crate::project::resolve_base;
use crate::templates::DEFAULT_OPYS_CONFIG;
use crate::Ctx;

/// Write the default `opys.toml` to the inventory base, without overwriting.
pub fn init(ctx: &Ctx) -> Result<()> {
    let base = resolve_base(&ctx.root, &ctx.dir);
    std::fs::create_dir_all(&base)?;
    let path = base.join("opys.toml");
    if path.exists() {
        println!("{} already exists; leaving it untouched", path.display());
    } else {
        std::fs::write(&path, DEFAULT_OPYS_CONFIG)?;
        println!(
            "created {} — edit it to model your document types",
            path.display()
        );
    }
    Ok(())
}
