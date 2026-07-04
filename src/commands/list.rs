//! `opys list` — slice the inventory with one SQL SELECT over the corpus
//! store: a JOIN per `--tag`/`--field` filter (AND semantics), column
//! predicates for `--type`/`--status`, DISTINCT against multi-row matches,
//! ordered by load (path) order.

use std::collections::BTreeMap;

use gluesql::prelude::ParamLiteral;

use crate::cli::ListFormat;
use crate::commands::parse_field_filters;
use crate::error::Result;
use crate::store::{g_i64, g_str, IntoParam, Store};
use crate::Ctx;

pub fn run(
    ctx: &Ctx,
    type_name: Option<&str>,
    tag: Option<&str>,
    status: Option<&str>,
    field: &[String],
    format: ListFormat,
) -> Result<()> {
    let prj = ctx.open()?;
    let filters = parse_field_filters(field)?;
    let (mut store, _) = Store::open(&prj)?;

    // Build the one SELECT: joins for tag/field filters, predicates for
    // type/status. Values always as parameters.
    let mut sql =
        String::from("SELECT DISTINCT d.dkey, d.id, d.status, d.title, d.path FROM docs d");
    let mut wheres: Vec<String> = Vec::new();
    let mut params: Vec<ParamLiteral> = Vec::new();
    let mut n = 0usize;
    let mut next = |params: &mut Vec<ParamLiteral>, v: String| {
        params.push(v.into_param());
        n += 1;
        format!("${n}")
    };

    if let Some(q) = tag {
        // Matches an exact tag or any tag whose key equals the query.
        let p = next(&mut params, q.to_string());
        sql.push_str(&format!(
            " JOIN tags tq ON tq.dkey = d.dkey AND (tq.tag = {p} OR tq.key = {p})"
        ));
    }
    if let Some(t) = type_name {
        let p = next(&mut params, t.to_string());
        wheres.push(format!("d.type = {p}"));
    }
    if let Some(s) = status {
        let p = next(&mut params, s.to_string());
        wheres.push(format!("d.status = {p}"));
    }
    for (i, (key, want)) in filters.iter().enumerate() {
        // `--field key=value` matches wherever the key lives: a core column
        // (right-typed), the tags list (element equality), or an fm_fields
        // fidelity/element row (scalar_str coercion) — same semantics as the
        // old in-memory `field_matches`.
        let pk = next(&mut params, key.clone());
        let pv = next(&mut params, want.clone());
        sql.push_str(&format!(
            " LEFT JOIN fm_fields f{i} ON f{i}.dkey = d.dkey AND f{i}.key = {pk} AND f{i}.value = {pv}"
        ));
        let mut arms = vec![format!("f{i}.value IS NOT NULL")];
        match key.as_str() {
            "id" | "status" | "created" | "updated" => {
                arms.push(format!("d.{key} = {pv}"));
            }
            "tags" => {
                sql.push_str(&format!(
                    " LEFT JOIN tags tf{i} ON tf{i}.dkey = d.dkey AND tf{i}.tag = {pv}"
                ));
                arms.push(format!("tf{i}.tag IS NOT NULL"));
            }
            _ => {}
        }
        wheres.push(format!("({})", arms.join(" OR ")));
    }
    if !wheres.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&wheres.join(" AND "));
    }
    sql.push_str(" ORDER BY d.dkey");

    let (_, rows) = store.select(&sql, params)?;

    // Table format needs each doc's tags, joined in list order.
    let doc_tags: BTreeMap<i64, Vec<String>> = if matches!(format, ListFormat::Table) {
        let (_, trows) = store.select("SELECT dkey, tag FROM tags ORDER BY dkey, seq", vec![])?;
        let mut m: BTreeMap<i64, Vec<String>> = BTreeMap::new();
        for r in trows {
            if let (Some(k), Some(t)) = (g_i64(&r[0]), g_str(&r[1])) {
                m.entry(k).or_default().push(t);
            }
        }
        m
    } else {
        BTreeMap::new()
    };

    for r in rows {
        let dkey = g_i64(&r[0]).unwrap_or(0);
        let id = g_str(&r[1]).unwrap_or_default();
        let status = g_str(&r[2]).unwrap_or_default();
        let title = g_str(&r[3]).unwrap_or_default();
        let relpath = g_str(&r[4]).unwrap_or_default();
        match format {
            ListFormat::Ids => println!("{id}"),
            ListFormat::Paths => println!("{}", prj.base.join(relpath).display()),
            ListFormat::Table => {
                let tags = doc_tags
                    .get(&dkey)
                    .map(|ts| ts.join(", "))
                    .unwrap_or_default();
                println!("{id}  [{status}]  ({tags})  {title}");
            }
        }
    }
    Ok(())
}
