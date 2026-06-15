use crate::commands::{maybe_sync, split_csv};
use crate::error::{usage, Result};
use crate::feature::Feature;
use crate::frontmatter::Frontmatter;
use crate::project::{self, Project};
use crate::Ctx;

pub fn run(
    ctx: &Ctx,
    title: &str,
    tags: &str,
    status: &str,
    reason: Option<&str>,
    fields: &[String],
) -> Result<()> {
    let prj = Project::open(&ctx.root, &ctx.dir)?;
    let (feats, _) = prj.load();
    // The ID sequence is global, so work items count toward the next number too.
    let (wis, _) = if prj.wi_cfg.is_some() {
        prj.load_work_items()
    } else {
        (Vec::new(), Vec::new())
    };
    let id = prj.next_id(&feats, &wis);

    let tags = split_csv(tags);
    if tags.is_empty() {
        return Err(usage("at least one tag is required (--tags a,b)"));
    }

    // Enforce the status lifecycle at write time, the same way set-status and
    // verify do — `new` previously accepted any status and deferred these to
    // verify.
    let statuses = prj.cfg.statuses();
    if !statuses.iter().any(|s| s == status) {
        return Err(usage(format!(
            "unknown status {status:?} (allowed: {})",
            statuses.join(", ")
        )));
    }
    if status == "implemented" {
        return Err(usage(
            "cannot create a feature as implemented: a new file has no test plan yet \
             — create it as planned/partial, add a checked test-plan item, then run \
             `opys set-status <id> implemented`",
        ));
    }

    let mut fm = Frontmatter::new();
    fm.set_str("id", &id);
    fm.set_str("status", status);
    fm.set_tags(&tags);
    for kv in fields {
        let (k, v) = project::parse_field(kv)?;
        fm.insert(&k, v);
    }
    if status == "wontfix" {
        if let Some(r) = reason {
            fm.set_str("wontfix_reason", r);
        }
        if fm.wontfix_reason().is_none() {
            return Err(usage(
                "creating a feature as wontfix requires --reason \
                 (or --field wontfix_reason=...)",
            ));
        }
    }

    let body = format!("# {title}\n");
    let path = prj.path_for(&id);
    let feature = Feature {
        path: path.clone(),
        frontmatter: fm,
        body,
        title: title.to_string(),
    };
    std::fs::write(&path, feature.to_text())?;
    println!("{}", path.display());
    maybe_sync(ctx, &prj);
    Ok(())
}
