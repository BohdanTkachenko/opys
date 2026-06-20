//! `opys tags` — enumerate the distinct tags across the inventory (or just
//! their keys with `--keys`). Plain, sorted, one per line — easy to scan or
//! pipe into `opys list --tag`.

use std::collections::BTreeSet;

use crate::commands::tag_key;
use crate::error::Result;
use crate::Ctx;

/// Print every distinct tag (or, with `keys_only`, every distinct tag key) in
/// the inventory, sorted alphabetically, one per line.
pub fn run(ctx: &Ctx, keys_only: bool) -> Result<()> {
    let prj = ctx.open()?;
    let (docs, _) = prj.load_docs();

    let mut tags: BTreeSet<String> = BTreeSet::new();
    for d in &docs {
        for t in d.frontmatter.tags().unwrap_or_default() {
            if keys_only {
                tags.insert(tag_key(&t).to_string());
            } else {
                tags.insert(t);
            }
        }
    }

    for t in &tags {
        println!("{t}");
    }
    Ok(())
}
