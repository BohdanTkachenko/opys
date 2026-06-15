use std::collections::HashSet;

use crate::commands::maybe_sync;
use crate::error::{usage, Result};
use crate::Ctx;
use crate::{project, rules};

pub fn run(ctx: &Ctx, id: &str, status: &str, reason: Option<&str>) -> Result<()> {
    let prj = ctx.open()?;
    let pcfg = &prj.pcfg;
    let tname = pcfg
        .type_name_for_id(id)
        .ok_or_else(|| usage(format!("unrecognized id prefix in {id}")))?
        .to_string();
    let t = &pcfg.types[&tname];

    if !t.statuses.iter().any(|s| s == status) {
        return Err(usage(format!(
            "unknown status {status:?} for type '{tname}' (allowed: {})",
            t.statuses.join(", ")
        )));
    }
    if t.terminal_statuses.iter().any(|s| s == status) {
        return Err(usage(format!(
            "{status} is terminal — use `opys close {id}` to reach it"
        )));
    }

    let (mut docs, _) = prj.load_docs();
    let doc_ids: HashSet<String> = docs
        .iter()
        .filter_map(|d| d.id())
        .map(str::to_string)
        .collect();
    let d = prj.find_mut(&mut docs, id)?;

    // `--reason` sets the conventional `<status>_reason` field.
    if let Some(r) = reason {
        d.frontmatter.set_str(&format!("{status}_reason"), r);
    }
    d.frontmatter.set_str("status", status);

    // Enforce the engine at write time, exactly as verify does.
    let problems = rules::evaluate(pcfg, &tname, status, &d.frontmatter, &d.body, &doc_ids);
    if !problems.is_empty() {
        return Err(usage(format!(
            "cannot set {id} to {status}: {}",
            problems.join("; ")
        )));
    }

    project::write_doc(d)?;
    println!("{id} -> {status}");
    maybe_sync(ctx, &prj);
    Ok(())
}
