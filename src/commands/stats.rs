//! `opys stats` — per-type status breakdown (counts + percentages) and
//! test-plan / manual coverage. Pure read; no feature/type special-casing.

use crate::body;
use crate::doc::Doc;
use crate::error::Result;
use crate::project_config::SectionKind;
use crate::Ctx;

pub fn run(ctx: &Ctx) -> Result<()> {
    let prj = ctx.open()?;
    let (docs, _) = prj.load_docs();
    let pcfg = &prj.pcfg;

    println!("documents: {}", docs.len());

    // Per type: total, then each present status with count and percent of the type.
    for (tname, t) in &pcfg.types {
        let group: Vec<&Doc> = docs
            .iter()
            .filter(|d| d.id().and_then(|id| pcfg.type_name_for_id(id)) == Some(tname.as_str()))
            .collect();
        let total = group.len();
        if total == 0 {
            continue;
        }
        println!("\n{tname}: {total}");
        for status in &t.statuses {
            let count = group
                .iter()
                .filter(|d| d.status() == Some(status.as_str()))
                .count();
            if count > 0 {
                let pct = (count as f64 / total as f64 * 100.0).round() as u32;
                println!("  {status:<16} {count:>4}  {pct:>3}%");
            }
        }
    }

    // Test-plan / manual coverage across every type's relevant sections.
    let mut uncovered = 0usize;
    let mut manual_total = 0usize;
    let mut manual_uncovered = 0usize;
    for d in &docs {
        let Some(tname) = d.id().and_then(|id| pcfg.type_name_for_id(id)) else {
            continue;
        };
        for sec in &pcfg.types[tname].sections {
            match sec.kind {
                SectionKind::TestPlan => {
                    uncovered += body::checklist_items(&d.body, &sec.heading)
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
    println!("\nuncovered test-plan items: {uncovered}");
    println!("manual verification items: {manual_total}");
    println!("manual items without automated coverage: {manual_uncovered}");
    Ok(())
}
