//! The derived tables (`fields`, `sections`) and plan-guarded user SQL.
//!
//! `fields`/`sections` mirror the `[[stats]]` corpus-table shapes so users
//! learn one vocabulary; they are rebuilt from scratch here (never written by
//! commands), immediately before any user SQL runs. User SQL executes against
//! the LIVE store, so it is statement-guarded on the plan: read SQL must be all
//! `Query` (SELECT) and `--write` SQL all DML (INSERT/UPDATE/DELETE) — a mixed
//! compound (`DELETE …; SELECT 1`, `UPDATE …; DROP TABLE docs`) is rejected
//! before anything executes, not after.

use futures::executor::block_on;
use gluesql::core::ast::Statement;
use gluesql::prelude::Payload;
use serde_json::Value as Json;

use crate::commands::stats::{
    cell, json_scalar, section_json, structured_section_json, yaml_to_json,
};
use crate::error::Result;
use crate::project_config::{ProjectConfig, SectionKind};

use super::{IntoParam, Store};

impl Store {
    /// Rebuild the derived `fields`/`sections`/`blocks` tables from the current
    /// authoritative state. `fields`/`sections` mirror the `[[stats]]` corpus
    /// shapes; `blocks` decomposes each body into one row per `##` section
    /// (+ preamble) for querying into bodies.
    pub fn refresh_projections(&mut self, pcfg: &ProjectConfig) -> Result<()> {
        self.exec("DELETE FROM fields", vec![])?;
        self.exec("DELETE FROM sections", vec![])?;
        self.exec("DELETE FROM blocks", vec![])?;

        let docs = self.all_docs()?;
        let mut field_rows = Vec::new();
        let mut section_rows = Vec::new();
        let mut block_rows = Vec::new();
        for (_, d) in &docs {
            let Some(id) = d.id().map(str::to_string) else {
                continue;
            };
            // Body decomposition: one row per `##` section (+ any preamble), for
            // querying into document bodies. Independent of the doc's type.
            let secs: Vec<(String, String)> = crate::body::sections(&d.body)
                .into_iter()
                .filter_map(|(h, t)| {
                    let t = t.trim().to_string();
                    (!(h.is_empty() && t.is_empty())).then_some((h, t))
                })
                .collect();
            for (seq, (heading, text)) in secs.into_iter().enumerate() {
                block_rows.push(vec![
                    id.clone().into_param(),
                    (seq as i64).into_param(),
                    heading.into_param(),
                    text.into_param(),
                ]);
            }
            let Some(tname) = pcfg.type_name_for_id(&id) else {
                continue;
            };
            let t = &pcfg.types[tname];
            for fname in t.fields.keys() {
                if let Some(v) = d.frontmatter.get(fname) {
                    match yaml_to_json(v) {
                        Json::Array(items) => {
                            for it in &items {
                                field_rows.push(vec![
                                    id.clone().into_param(),
                                    fname.clone().into_param(),
                                    json_scalar(it).into_param(),
                                ]);
                            }
                        }
                        other => field_rows.push(vec![
                            id.clone().into_param(),
                            fname.clone().into_param(),
                            json_scalar(&other).into_param(),
                        ]),
                    }
                }
            }
            for sec in &t.sections {
                let s = if sec.kind == SectionKind::Structured {
                    structured_section_json(d, sec.structure.as_deref(), &sec.heading)
                } else {
                    section_json(d, sec.kind, &sec.heading)
                };
                if let Some(s) = s {
                    section_rows.push(vec![
                        id.clone().into_param(),
                        sec.heading.clone().into_param(),
                        s["kind"].as_str().unwrap_or("").to_string().into_param(),
                        s["items"].as_i64().unwrap_or(0).into_param(),
                        s["unchecked"].as_i64().unwrap_or(0).into_param(),
                    ]);
                }
            }
        }
        self.insert_batch("fields", 3, field_rows)?;
        self.insert_batch("sections", 5, section_rows)?;
        self.insert_batch("blocks", 4, block_rows)?;
        Ok(())
    }

    /// Execute user-supplied SQL read-only against the live store. Every
    /// statement must be a SELECT — checked on the *plan*, before anything
    /// executes. Returns (labels, stringified rows); `Err` carries a
    /// human-readable problem.
    pub fn run_user_query(
        &mut self,
        sql: &str,
        params: &[String],
    ) -> std::result::Result<(Vec<String>, Vec<Vec<String>>), String> {
        let bound: Vec<_> = params.iter().map(|p| p.clone().into_param()).collect();
        let stmts = block_on(self.glue.plan_with_params(sql, bound))
            .map_err(|e| format!("query failed ({e})"))?;
        if stmts.is_empty() {
            return Err("query produced no result set".to_string());
        }
        for s in &stmts {
            if !matches!(s, Statement::Query(_)) {
                return Err(format!("query must be a SELECT (got {})", stmt_kind(s)));
            }
        }
        let mut last = None;
        for s in &stmts {
            last = Some(
                block_on(self.glue.execute_stmt(s)).map_err(|e| format!("query failed ({e})"))?,
            );
        }
        match last {
            Some(Payload::Select { labels, rows }) => {
                let rows = rows.iter().map(|r| r.iter().map(cell).collect()).collect();
                Ok((labels, rows))
            }
            other => Err(format!("query produced no result set ({other:?})")),
        }
    }

    /// Execute user-supplied write SQL (INSERT/UPDATE/DELETE) against the live
    /// store, returning a one-line summary of the row counts. The caller is
    /// responsible for gating on `verify` and flushing — this only mutates the
    /// in-memory store. `Err` carries a human-readable problem.
    ///
    /// Every statement must be a DML mutation — checked on the *plan*, before
    /// anything executes, so a compound like `INSERT …; DROP TABLE docs` is
    /// rejected outright. This keeps schema-level statements (CREATE/DROP/ALTER
    /// TABLE, indexes) off the authoritative tables the store and flush depend
    /// on; the verify gate alone can't catch them (an empty or reshaped table
    /// has no verify problems).
    pub fn run_user_write(
        &mut self,
        sql: &str,
        params: &[String],
    ) -> std::result::Result<String, String> {
        let bound: Vec<_> = params.iter().map(|p| p.clone().into_param()).collect();
        let stmts = block_on(self.glue.plan_with_params(sql, bound))
            .map_err(|e| format!("statement failed ({e})"))?;
        if stmts.is_empty() {
            return Err("no statements to run".to_string());
        }
        for s in &stmts {
            if !matches!(
                s,
                Statement::Insert { .. } | Statement::Update { .. } | Statement::Delete { .. }
            ) {
                return Err(format!(
                    "query --write allows only INSERT/UPDATE/DELETE (got {})",
                    stmt_kind(s)
                ));
            }
        }
        let mut payloads = Vec::with_capacity(stmts.len());
        for s in &stmts {
            payloads.push(
                block_on(self.glue.execute_stmt(s))
                    .map_err(|e| format!("statement failed ({e})"))?,
            );
        }
        let parts: Vec<String> = payloads
            .iter()
            .map(|p| match p {
                Payload::Insert(n) => format!("{n} inserted"),
                Payload::Update(n) => format!("{n} updated"),
                Payload::Delete(n) => format!("{n} deleted"),
                Payload::Select { rows, .. } => format!("{} selected", rows.len()),
                _ => "ok".to_string(),
            })
            .collect();
        Ok(parts.join(", "))
    }
}

/// A human-readable name for a rejected statement kind.
fn stmt_kind(s: &Statement) -> &'static str {
    match s {
        Statement::Insert { .. } => "INSERT",
        Statement::Update { .. } => "UPDATE",
        Statement::Delete { .. } => "DELETE",
        Statement::CreateTable { .. } => "CREATE TABLE",
        Statement::DropTable { .. } => "DROP TABLE",
        Statement::AlterTable { .. } => "ALTER TABLE",
        Statement::CreateIndex { .. } => "CREATE INDEX",
        Statement::DropIndex { .. } => "DROP INDEX",
        Statement::Query(_) => "SELECT",
        _ => "an unsupported statement",
    }
}
