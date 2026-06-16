//! `opys config …` — project-config commands.
//!
//! Currently just `config init`, which scaffolds the opinionated default
//! `opys.toml` for the upcoming universal typed-document engine. Nothing reads
//! that file yet; this only generates it so the config shape can be reviewed and
//! iterated on. Future subcommands (`config validate`, `config show`) belong
//! here too.

use crate::error::{usage, Result};
use crate::project::{find_root, start_dir};
use crate::project_config::ProjectConfig;
use crate::templates::DEFAULT_OPYS_CONFIG;
use crate::Ctx;

/// Write the default `opys.toml` at the project root, without overwriting.
pub fn init(ctx: &Ctx) -> Result<()> {
    let root = start_dir(&ctx.root)?;
    std::fs::create_dir_all(&root)?;
    let path = root.join("opys.toml");
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

/// Parse `opys.toml` (found by searching upward) and report any well-formedness
/// problems. Returns `1` when the config has problems (mirroring `verify`), `0`
/// when clean; a missing file or TOML syntax error surfaces as a hard error.
pub fn validate(ctx: &Ctx) -> Result<i32> {
    let start = start_dir(&ctx.root)?;
    let root = find_root(&start).ok_or_else(|| {
        usage(format!(
            "no opys.toml found in {} or any parent directory — run `opys config init`",
            start.display()
        ))
    })?;
    let cfg = ProjectConfig::load(&root.join("opys.toml"))?;
    let problems = cfg.validate();
    if problems.is_empty() {
        println!(
            "config: OK ({} types, {} rules)",
            cfg.types.len(),
            cfg.rules.len()
        );
        Ok(0)
    } else {
        eprintln!("config: {} problem(s)", problems.len());
        for p in &problems {
            eprintln!("  {p}");
        }
        Ok(1)
    }
}
