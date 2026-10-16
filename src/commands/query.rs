//! `opys query` — run SQL over the live corpus store.
//!
//! Read-only, always: a plan-guarded SELECT (nothing else can execute), and the
//! command never flushes, so the files on disk are unreachable from here. Every
//! write goes through a lifecycle command instead (ADR-0051) — bulk edits
//! compose by piping `opys list --format ids` into one of them, which is why
//! there is no `--write` and no writable projection here.

use std::io::Read as _;

use crate::error::{usage, Result};
use crate::Ctx;

use super::stats;

pub fn run(ctx: &Ctx, sql: &str, plain: bool, stdin_value: bool) -> Result<()> {
    let prj = ctx.open()?;
    let sql_from_stdin = sql == "-";
    let sql = if sql_from_stdin {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        sql.to_string()
    };
    if sql.trim().is_empty() {
        return Err(usage("query: empty SQL"));
    }
    // `--stdin` binds stdin to `$1` in the SQL (large/multi-line values without
    // SQL-escaping); it can't also read the SQL itself from stdin.
    let params: Vec<String> = if stdin_value {
        if sql_from_stdin {
            return Err(usage(
                "query: --stdin cannot combine with reading the SQL from stdin (-)",
            ));
        }
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        vec![buf.trim_end().to_string()]
    } else {
        Vec::new()
    };

    // Unparsable docs are warnings — query the parsable subset (list parity).
    let (mut store, errors) = ctx.load(&prj)?;
    for e in &errors {
        eprintln!("warning: {e}");
    }
    store.refresh_projections(&prj.pcfg)?;

    let (labels, rows) = store.run_user_query(&sql, &params).map_err(usage)?;
    stats::print_markdown(&stats::table_body(&labels, &rows), plain)
}
