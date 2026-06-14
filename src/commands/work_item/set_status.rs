use crate::commands::maybe_sync;
use crate::error::{usage, Result};
use crate::project;
use crate::Ctx;

pub fn run(ctx: &Ctx, id: &str, status: &str, reason: Option<&str>) -> Result<()> {
    let prj = ctx.open()?;
    let statuses = prj.require_wi_cfg()?.statuses();
    let (mut items, _) = prj.load_work_items();
    let w = prj.find_wi_mut(&mut items, id)?;

    if !statuses.iter().any(|s| s == status) {
        return Err(usage(format!(
            "unknown status {status:?} (allowed: {})",
            statuses.join(", ")
        )));
    }
    if status == "done" {
        return Err(usage(
            "use `opys work-item close` to finish a work item (status 'done' is terminal)",
        ));
    }
    if status == "blocked" {
        let has_reason = reason.is_some() || w.frontmatter.get_str("blocked_reason").is_some();
        if !has_reason {
            return Err(usage("blocked requires --reason"));
        }
        if let Some(r) = reason {
            w.frontmatter.set_str("blocked_reason", r);
        }
    }

    w.frontmatter.set_str("status", status);
    project::write_work_item(w)?;
    println!("{id} -> {status}");
    maybe_sync(ctx, &prj);
    Ok(())
}
