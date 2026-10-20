//! A read-only role queue, with fresh verification for escalation attention.
use crate::{
    commands::{print_lines, verify},
    doc::Doc,
    error::{usage, OpysError, Result},
    observe,
    queue::{self, QueueQuery, QueueResult},
    roles, Ctx,
};

pub fn run(
    ctx: &Ctx,
    role: Option<&str>,
    type_name: Option<&str>,
    limit: usize,
    json: bool,
) -> Result<()> {
    let mut prj = ctx.open()?;
    // The actor opens the canonical primary root. Use that same spelling for
    // medium findings too, including parse errors, even when invoked at '.'.
    prj.root = prj.root.canonicalize()?;
    prj.base = prj.pcfg.resolve_base(&prj.root)?;
    let env_role = std::env::var("OPYS_ROLE").ok();
    let role = role
        .or(env_role.as_deref())
        .ok_or_else(|| usage(roles::missing_role(&prj.pcfg)))?;
    let q = QueueQuery {
        role: role.to_string(),
        type_filter: type_name.map(str::to_owned),
        limit,
    };
    q.validate(&prj.pcfg).map_err(usage)?;
    let (mut store, parse_errors) = ctx.load(&prj)?;
    let docs: Vec<Doc> = store.all_docs()?.into_iter().map(|(_, d)| d).collect();
    let mut findings = verify::collect_findings(&prj, &docs, parse_errors);
    findings.extend(verify::spelling_findings(&docs, ctx.backend.as_ref()));
    let result = queue::build(&prj, &docs, &q, observe::now(), &findings);
    for notice in &result.notices {
        eprintln!("warning: {notice}");
    }
    if json {
        return print_lines([serde_json::to_string_pretty(&result)
            .map_err(|e| OpysError::Store(format!("serializing the queue failed: {e}")))?]);
    }
    print_lines(text(&result))
}

pub fn text(q: &QueueResult) -> Vec<String> {
    let mut lines = vec![if q.total == 0 {
        format!("{}: nothing waiting", q.role)
    } else {
        format!("{}: {} waiting", q.role, q.total)
    }];
    for g in &q.groups {
        if g.total == 0 && g.in_progress.is_empty() && g.blocked.is_empty() && g.orphaned.is_empty()
        {
            continue;
        }
        lines.push(String::new());
        lines.push(format!(
            "{} · {}  ({})",
            g.type_name,
            g.status,
            g.owners.join(", ")
        ));
        for i in g.items.iter().chain(&g.in_progress) {
            let since = i
                .since
                .as_deref()
                .map(|s| format!("  since {}", s.get(..10).unwrap_or(s)))
                .unwrap_or_default();
            let priority = i
                .item
                .effective_priority
                .map(|p| format!("  p{p}"))
                .unwrap_or_default();
            let override_mark = if i.owner_override { "  (override)" } else { "" };
            let reference = i
                .item
                .reference
                .as_ref()
                .map(|r| format!("  ref {r}"))
                .unwrap_or_default();
            let claim = i
                .item
                .claim
                .as_ref()
                .map(|c| {
                    format!(
                        "  claimed by {} (age {})",
                        c.holder,
                        c.age_seconds
                            .map(|s| format!("{s}s"))
                            .unwrap_or_else(|| "unknown".into())
                    )
                })
                .unwrap_or_default();
            lines.push(format!(
                "  {}  {}{since}{priority}{override_mark}{reference}{claim}",
                i.item.id, i.item.title
            ));
        }
        if !g.blocked.is_empty() {
            lines.push(format!(
                "  {} blocked: {}",
                g.blocked.len(),
                g.blocked
                    .iter()
                    .map(|b| format!("{} (by {})", b.id, b.blocked_by.join(", ")))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for o in &g.orphaned {
            lines.push(format!("  {}  orphaned: {}", o.id, o.reason));
        }
    }
    if !q.attention.is_empty() {
        lines.push("\nattention  (escalate_to)".to_string());
        for a in &q.attention {
            lines.push(format!(
                "  {}  {}  {}",
                a.id,
                a.status,
                a.reasons
                    .iter()
                    .map(|r| format!("{}: {}", r.kind, r.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
    }
    if !q.project_problems.is_empty() {
        lines.push("\nproject problems".to_string());
        lines.extend(q.project_problems.iter().map(|p| format!("  {p}")));
    }
    lines
}
