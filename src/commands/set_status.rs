use crate::body;
use crate::error::{usage, Result};
use crate::project::{self, Project};

pub fn run(root: &str, id: &str, status: &str, reason: Option<&str>) -> Result<()> {
    let prj = Project::open(root)?;
    let (mut feats, _) = prj.load();
    let statuses = prj.cfg.statuses();
    let f = prj.find_mut(&mut feats, id)?;

    if !statuses.iter().any(|s| s == status) {
        return Err(usage(format!(
            "unknown status {status:?} (allowed: {})",
            statuses.join(", ")
        )));
    }

    if status == "wontfix" {
        let has_reason = reason.is_some() || f.frontmatter.wontfix_reason().is_some();
        if !has_reason {
            return Err(usage("wontfix requires --reason"));
        }
        if let Some(r) = reason {
            f.frontmatter.set_str("wontfix_reason", r);
        }
    }

    if status == "implemented" && !body::test_plan_items(&f.body).iter().any(|i| i.checked) {
        return Err(usage(
            "cannot mark implemented — no checked test-plan item \
             (add the test, check the item, then retry)",
        ));
    }

    f.frontmatter.set_str("status", status);
    project::write_feature(f)?;
    println!("{id} -> {status}");
    Ok(())
}
