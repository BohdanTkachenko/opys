//! `opys query` — run a read-only SQL SELECT over the live corpus store and
//! print the result as a table. The statement is plan-guarded (SELECT only;
//! nothing executes otherwise), and the command never flushes, so the files
//! are unreachable by construction.

use std::io::Read as _;

use crate::error::{usage, Result};
use crate::store::Store;
use crate::Ctx;

use super::stats;

pub fn run(ctx: &Ctx, sql: &str, plain: bool) -> Result<()> {
    let prj = ctx.open()?;
    let sql = if sql == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        sql.to_string()
    };
    if sql.trim().is_empty() {
        return Err(usage("query: empty SQL"));
    }

    // Unparsable docs are warnings — query the parsable subset (list parity).
    let (mut store, errors) = Store::open(&prj)?;
    for e in &errors {
        eprintln!("warning: {e}");
    }
    store.refresh_projections(&prj.pcfg)?;
    let (labels, rows) = store.run_user_query(&sql).map_err(usage)?;
    stats::print_markdown(&stats::table_body(&labels, &rows), plain);
    Ok(())
}
