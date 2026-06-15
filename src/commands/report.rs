use std::collections::HashMap;

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

    let docs_of = |tname: &str| -> Vec<&Doc> {
        docs.iter()
            .filter(|d| d.id().and_then(|id| pcfg.type_name_for_id(id)) == Some(tname))
            .collect()
    };

    // Per-type status counts.
    for (tname, t) in &pcfg.types {
        let group = docs_of(tname);
        if group.is_empty() {
            continue;
        }
        println!("{tname}: {}", group.len());
        let mut by: HashMap<&str, usize> = HashMap::new();
        for d in &group {
            *by.entry(d.status().unwrap_or("")).or_default() += 1;
        }
        for s in &t.statuses {
            if let Some(&c) = by.get(s.as_str()) {
                if c > 0 {
                    println!("  {s}: {c}");
                }
            }
        }
    }

    // Feature-parity percentages, when enabled.
    if pcfg.report.parity {
        if let Some(feat_type) = pcfg.types.keys().find(|k| k.as_str() == "feature") {
            let group = docs_of(feat_type);
            let n = group.len();
            let count = |s: &str| group.iter().filter(|d| d.status() == Some(s)).count();
            let implemented = count("implemented");
            let wontfix = count("wontfix");
            if n > 0 {
                println!(
                    "parity (impl / all): {:.1}%",
                    100.0 * implemented as f64 / n as f64
                );
            }
            if n > wontfix {
                println!(
                    "parity (impl / all minus wontfix): {:.1}%",
                    100.0 * implemented as f64 / (n - wontfix) as f64
                );
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
    println!("uncovered test-plan items: {uncovered}");
    println!("manual verification items: {manual_total}");
    println!("manual items without automated coverage: {manual_uncovered}");
    Ok(())
}
