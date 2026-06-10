use std::io::Write;

use crate::error::Result;
use crate::project::Project;

pub fn run(root: &str, id: &str) -> Result<()> {
    let prj = Project::open(root)?;
    let (feats, _) = prj.load();
    let f = prj.find(&feats, id)?;
    let text = std::fs::read_to_string(&f.path)?;
    // Print verbatim, without forcing a trailing newline.
    print!("{text}");
    std::io::stdout().flush()?;
    Ok(())
}
