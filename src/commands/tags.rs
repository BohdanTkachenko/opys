//! `opys tags` — enumerate the distinct tags across the inventory (or just
//! their keys with `--keys`). Plain, sorted, one per line — easy to scan or
//! pipe into `opys list --tag`.

use crate::error::Result;
use crate::store::{g_str, Store};
use crate::Ctx;

/// Print every distinct tag (or, with `keys_only`, every distinct tag key) in
/// the inventory, sorted alphabetically, one per line.
pub fn run(ctx: &Ctx, keys_only: bool) -> Result<()> {
    let prj = ctx.open()?;
    let (mut store, _) = Store::open(&prj)?;
    let sql = if keys_only {
        "SELECT DISTINCT key FROM tags ORDER BY key"
    } else {
        "SELECT DISTINCT tag FROM tags ORDER BY tag"
    };
    let (_, rows) = store.select(sql, vec![])?;
    for r in rows {
        if let Some(t) = g_str(&r[0]) {
            println!("{t}");
        }
    }
    Ok(())
}
