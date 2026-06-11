use std::collections::HashMap;

use crate::body;
use crate::error::Result;
use crate::Ctx;

pub fn run(ctx: &Ctx) -> Result<()> {
    let prj = ctx.open()?;
    let (feats, _) = prj.load();
    let n = feats.len();

    let mut by: HashMap<String, usize> = HashMap::new();
    for f in &feats {
        *by.entry(f.status().unwrap_or("").to_string()).or_default() += 1;
    }

    println!("features: {n}");
    for s in prj.cfg.statuses() {
        if let Some(&c) = by.get(&s) {
            if c > 0 {
                println!("  {s}: {c}");
            }
        }
    }

    if prj.cfg.parity {
        let implemented = *by.get("implemented").unwrap_or(&0);
        let wontfix = *by.get("wontfix").unwrap_or(&0);
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

    let uncovered: usize = feats
        .iter()
        .flat_map(|f| body::test_plan_items(&f.body))
        .filter(|i| !i.checked)
        .count();
    let manual: Vec<_> = feats
        .iter()
        .flat_map(|f| body::manual_items(&f.body))
        .collect();
    let manual_uncovered = manual.iter().filter(|m| m.uncovered()).count();
    println!("uncovered test-plan items: {uncovered}");
    println!("manual verification items: {}", manual.len());
    println!("manual items without automated coverage: {manual_uncovered}");
    Ok(())
}
