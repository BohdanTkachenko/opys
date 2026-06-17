//! `opys stats` — per-type status breakdown (counts + percentages) and
//! test-plan / manual coverage. Pure read; no feature/type special-casing.

use crate::body;
use crate::doc::Doc;
use crate::error::Result;
use crate::project_config::{ProjectConfig, SectionKind};
use crate::Ctx;

/// One status row within a type: its name, count, and percent of the type total.
pub struct StatusCount {
    pub status: String,
    pub count: usize,
    pub pct: u32,
}

/// Per-type breakdown: total documents and the present statuses.
pub struct TypeStats {
    pub name: String,
    pub total: usize,
    pub by_status: Vec<StatusCount>,
}

/// The computed stats over a set of documents — the shared data behind the CLI
/// `stats` command and the TUI stats screen (which feeds it a filtered slice).
pub struct StatsReport {
    pub total: usize,
    pub per_type: Vec<TypeStats>,
    pub uncovered_testplan: usize,
    pub manual_total: usize,
    pub manual_uncovered: usize,
}

/// Compute the stats report over `docs` (already filtered by the caller). Pure;
/// does not read the filesystem or print.
pub fn compute(pcfg: &ProjectConfig, docs: &[&Doc]) -> StatsReport {
    let mut per_type = Vec::new();
    for (tname, t) in &pcfg.types {
        let group: Vec<&&Doc> = docs
            .iter()
            .filter(|d| d.id().and_then(|id| pcfg.type_name_for_id(id)) == Some(tname.as_str()))
            .collect();
        let total = group.len();
        if total == 0 {
            continue;
        }
        let mut by_status = Vec::new();
        for status in &t.statuses {
            let count = group
                .iter()
                .filter(|d| d.status() == Some(status.as_str()))
                .count();
            if count > 0 {
                let pct = (count as f64 / total as f64 * 100.0).round() as u32;
                by_status.push(StatusCount {
                    status: status.clone(),
                    count,
                    pct,
                });
            }
        }
        per_type.push(TypeStats {
            name: tname.clone(),
            total,
            by_status,
        });
    }

    // Test-plan / manual coverage across every type's relevant sections.
    let mut uncovered_testplan = 0usize;
    let mut manual_total = 0usize;
    let mut manual_uncovered = 0usize;
    for d in docs {
        let Some(tname) = d.id().and_then(|id| pcfg.type_name_for_id(id)) else {
            continue;
        };
        for sec in &pcfg.types[tname].sections {
            match sec.kind {
                SectionKind::TestPlan => {
                    uncovered_testplan += body::checklist_items(&d.body, &sec.heading)
                        .iter()
                        .filter(|i| !i.checked)
                        .count();
                }
                SectionKind::Manual => {
                    for it in body::manual_items_in(&d.body, &sec.heading) {
                        manual_total += 1;
                        if it.uncovered() {
                            manual_uncovered += 1;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    StatsReport {
        total: docs.len(),
        per_type,
        uncovered_testplan,
        manual_total,
        manual_uncovered,
    }
}

pub fn run(ctx: &Ctx) -> Result<()> {
    let prj = ctx.open()?;
    let (docs, _) = prj.load_docs();
    let refs: Vec<&Doc> = docs.iter().collect();
    let r = compute(&prj.pcfg, &refs);

    println!("documents: {}", r.total);
    for ts in &r.per_type {
        println!("\n{}: {}", ts.name, ts.total);
        for sc in &ts.by_status {
            println!("  {:<16} {:>4}  {:>3}%", sc.status, sc.count, sc.pct);
        }
    }
    println!("\nuncovered test-plan items: {}", r.uncovered_testplan);
    println!("manual verification items: {}", r.manual_total);
    println!(
        "manual items without automated coverage: {}",
        r.manual_uncovered
    );
    Ok(())
}
