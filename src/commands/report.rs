use std::collections::HashMap;

use crate::body;
use crate::error::Result;
use crate::project::Project;

pub fn run(root: &str) -> Result<()> {
    let prj = Project::open(root)?;
    let (feats, _) = prj.load();
    let n = feats.len();

    let mut by: HashMap<String, usize> = HashMap::new();
    for f in &feats {
        *by.entry(f.status().unwrap_or("").to_string()).or_default() += 1;
    }
    let implemented = *by.get("implemented").unwrap_or(&0);
    let wontfix = *by.get("wontfix").unwrap_or(&0);

    println!("features: {n}");
    for s in prj.cfg.statuses() {
        if let Some(&c) = by.get(&s) {
            if c > 0 {
                println!("  {s}: {c}");
            }
        }
    }
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

    let uncovered: usize = feats
        .iter()
        .flat_map(|f| body::test_plan_items(&f.body))
        .filter(|i| !i.checked)
        .count();
    let manual: usize = feats
        .iter()
        .map(|f| body::manual_items(&f.body).len())
        .sum();
    println!("uncovered test-plan items: {uncovered}");
    println!("manual verification items: {manual}");
    Ok(())
}
