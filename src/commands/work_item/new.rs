use crate::commands::{maybe_sync, split_csv};
use crate::config;
use crate::error::{usage, Result};
use crate::frontmatter::Frontmatter;
use crate::project::{self, Project};
use crate::refs;
use crate::work_item::WorkItem;
use crate::Ctx;

#[allow(clippy::too_many_arguments)]
pub fn run(
    ctx: &Ctx,
    title: &str,
    type_name: &str,
    features: &str,
    status: &str,
    tags: &str,
    reason: Option<&str>,
    fields: &[String],
) -> Result<()> {
    let prj = Project::open(&ctx.root, &ctx.dir)?;
    let wc = prj.require_wi_cfg()?;
    let statuses = wc.statuses();

    let wtype = config::type_by_name(type_name)
        .ok_or_else(|| usage(format!("unknown work-item type {type_name:?}")))?;
    let sections = wtype.required_sections(&wc.required_sections);

    let (feats, _) = prj.load();
    let (live, _) = prj.load_work_items();
    let id = prj.next_id_for_prefix(wtype.prefix, &feats, &live);

    // Every work item must link at least one existing feature.
    let feature_ids = split_csv(features);
    if feature_ids.is_empty() {
        return Err(usage(
            "at least one feature is required (--features FEAT-0001,...)",
        ));
    }
    let mut references = Vec::new();
    for fid in &feature_ids {
        let f = prj
            .find(&feats, fid)
            .map_err(|_| usage(format!("feature {fid} does not exist")))?;
        references.push((fid.clone(), f.title.clone()));
    }

    if !statuses.iter().any(|s| s == status) {
        return Err(usage(format!(
            "unknown status {status:?} (allowed: {})",
            statuses.join(", ")
        )));
    }
    if status == "done" {
        return Err(usage(
            "cannot create a work item as done — use `opys work-item close` to finish one",
        ));
    }

    let tags = split_csv(tags);

    let mut fm = Frontmatter::new();
    fm.set_str("id", &id);
    fm.set_str("status", status);
    if !tags.is_empty() {
        fm.set_tags(&tags);
    }
    refs::set(&mut fm, &references);
    for kv in fields {
        let (k, v) = project::parse_field(kv)?;
        fm.insert(&k, v);
    }
    if status == "blocked" {
        if let Some(r) = reason {
            fm.set_str("blocked_reason", r);
        }
        if fm.get_str("blocked_reason").is_none() {
            return Err(usage("creating a work item as blocked requires --reason"));
        }
    }

    // Scaffold the effective required sections for this type (the configured
    // baseline plus any per-type extras, e.g. a bug's `## Reproduction`).
    let mut body = format!("# {title}\n");
    for section in &sections {
        body.push_str(&format!("\n## {section}\n"));
        if section == "Tasks" {
            body.push_str("- [ ] First task\n");
        }
    }
    let path = prj.wi_path_for(&id);
    let wi = WorkItem {
        path: path.clone(),
        frontmatter: fm,
        body,
        title: title.to_string(),
    };
    std::fs::write(&path, wi.to_text())?;
    println!("{}", path.display());
    maybe_sync(ctx, &prj);
    Ok(())
}
