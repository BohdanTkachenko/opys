//! The derived tables (`fields`, `sections`) and plan-guarded user SQL.
//!
//! `fields`/`sections` mirror the `[[stats]]` corpus-table shapes so users
//! learn one vocabulary; they are rebuilt from scratch here (never written by
//! commands), immediately before any user SQL runs. User SQL executes against
//! the LIVE store, so it is statement-guarded: every planned statement must be
//! a `Query` (SELECT) — a `DELETE …; SELECT 1` compound is rejected before
//! anything executes, not after.

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
    /// Rebuild the derived `fields`/`sections` tables from the current
    /// authoritative state (same shapes and value coercions as the `[[stats]]`
    /// corpus tables).
    pub fn refresh_projections(&mut self, pcfg: &ProjectConfig) -> Result<()> {
        self.exec("DELETE FROM fields", vec![])?;
        self.exec("DELETE FROM sections", vec![])?;

        let docs = self.all_docs()?;
        let mut field_rows = Vec::new();
        let mut section_rows = Vec::new();
        for (_, d) in &docs {
            let Some(id) = d.id().map(str::to_string) else {
                continue;
            };
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
        Ok(())
    }

    /// Execute user-supplied SQL read-only against the live store. Every
    /// statement must be a SELECT — checked on the *plan*, before anything
    /// executes. Returns (labels, stringified rows); `Err` carries a
    /// human-readable problem.
    pub fn run_user_query(
        &mut self,
        sql: &str,
    ) -> std::result::Result<(Vec<String>, Vec<Vec<String>>), String> {
        let stmts = block_on(self.glue.plan(sql)).map_err(|e| format!("query failed ({e})"))?;
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
    pub fn run_user_write(&mut self, sql: &str) -> std::result::Result<String, String> {
        let payloads =
            block_on(self.glue.execute(sql)).map_err(|e| format!("statement failed ({e})"))?;
        if payloads.is_empty() {
            return Err("no statements to run".to_string());
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
        _ => "a non-SELECT statement",
    }
}
