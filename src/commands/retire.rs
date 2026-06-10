use std::io::Write;

use crate::commands::today;
use crate::error::Result;
use crate::project::Project;

pub fn run(root: &str, id: &str, reason: &str) -> Result<()> {
    let prj = Project::open(root)?;
    let (feats, _) = prj.load();
    let f = prj.find(&feats, id)?;
    let path = f.path.clone();

    let rp = prj.fdir.join("_retired.txt");
    let mut fh = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&rp)?;
    writeln!(fh, "{id}  # retired {}: {reason}", today())?;

    std::fs::remove_file(&path)?;
    println!("retired {id} (ID will never be reused)");
    Ok(())
}
