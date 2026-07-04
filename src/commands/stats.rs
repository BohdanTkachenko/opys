//! `opys stats` — render the project's configured `[[stats]]` sections. Each
//! stat is a single SQL query (`sql`) run against an in-memory, throwaway
//! relational view of the corpus (tables `docs`, `tags`, `sections`, `fields`);
//! its result set is rendered as a markdown table. Pure read; no feature/type
//! special-casing. The SQL engine is GlueSQL (pure Rust, in-memory).

use std::io::IsTerminal;

use futures::executor::block_on;
use gluesql::prelude::{Glue, MemoryStorage, Payload, Value as GlueValue};
use serde_json::{json, Map, Value};

use crate::body;
use crate::doc::Doc;
use crate::error::{usage, Result};
use crate::project_config::{ProjectConfig, SectionKind, StatSpec};
use crate::Ctx;

/// Split a tag into its key and optional value at the first `:` or `=`. A plain
/// tag has no value (`osc` → `("osc", None)`); a keyed tag splits at the first
/// separator (`area:parsing` → `("area", Some("parsing"))`).
fn split_tag(t: &str) -> (&str, Option<&str>) {
    match t.find([':', '=']) {
        Some(i) => (&t[..i], Some(&t[i + 1..])),
        None => (t, None),
    }
}

/// Sum the lengths of all top-level array fields in a JSON object, giving the
/// total number of items extracted from a structured section.
fn count_array_items(data: &Value) -> usize {
    match data {
        Value::Object(map) => map
            .values()
            .filter_map(|v| v.as_array())
            .map(|arr| arr.len())
            .sum(),
        Value::Array(arr) => arr.len(),
        _ => 0,
    }
}

/// A YAML frontmatter value converted to JSON (best-effort; unrepresentable
/// values become `null`).
pub(crate) fn yaml_to_json(v: &serde_norway::Value) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// The projected JSON for a non-structured section present in `d`, or `None`
/// when the section is absent/empty. Mirrors the countable-item logic behind
/// coverage stats. Structured sections are handled by `structured_section_json`.
pub(crate) fn section_json(d: &Doc, kind: SectionKind, heading: &str) -> Option<Value> {
    match kind {
        SectionKind::Checklist => {
            let items = body::checklist_items(&d.body, heading);
            if items.is_empty() {
                return None;
            }
            let unchecked = items.iter().filter(|i| !i.checked).count();
            Some(json!({ "kind": "checklist", "items": items.len(), "unchecked": unchecked }))
        }
        SectionKind::Log => {
            let content = body::section(&d.body, heading);
            let count = content.lines().filter(|l| l.starts_with("- ")).count();
            if count == 0 {
                return None;
            }
            Some(json!({ "kind": "log", "items": count }))
        }
        SectionKind::Structured | SectionKind::Prose => None,
    }
}

/// Countable JSON for a `structured` section: parse its schema, extract the
/// section body, and count the extracted array items.
pub(crate) fn structured_section_json(
    d: &Doc,
    structure: Option<&str>,
    heading: &str,
) -> Option<Value> {
    if !body::has_section(&d.body, heading) {
        return None;
    }
    let src = structure?;
    let schema = crate::mdprism::parse_schema(src).ok()?;
    let content = body::section(&d.body, heading);
    let data = schema.extract(&content).ok()?;
    let count = count_array_items(&data);
    if count == 0 {
        return None;
    }
    Some(json!({ "kind": "structured", "items": count }))
}

/// Project one document to the JSON object the corpus tables are built from.
fn doc_json(pcfg: &ProjectConfig, d: &Doc) -> Value {
    let id = d.id().unwrap_or("");
    let tname = d.id().and_then(|i| pcfg.type_name_for_id(i));
    let tags = d.frontmatter.tags().unwrap_or_default();

    let mut fields = Map::new();
    let mut sections = Map::new();
    if let Some(tn) = tname {
        let t = &pcfg.types[tn];
        for fname in t.fields.keys() {
            if let Some(v) = d.frontmatter.get(fname) {
                fields.insert(fname.clone(), yaml_to_json(v));
            }
        }
        for sec in &t.sections {
            let s = if sec.kind == SectionKind::Structured {
                structured_section_json(d, sec.structure.as_deref(), &sec.heading)
            } else {
                section_json(d, sec.kind, &sec.heading)
            };
            if let Some(s) = s {
                sections.insert(sec.heading.clone(), s);
            }
        }
    }

    json!({
        "id": id,
        "num": if id.is_empty() { 0 } else { crate::refs::id_number(id) },
        "type": tname,
        "status": d.status(),
        "title": d.title,
        "created": d.frontmatter.get_str("created"),
        "updated": d.frontmatter.get_str("updated"),
        "tags": tags,
        "fields": Value::Object(fields),
        "sections": Value::Object(sections),
    })
}

/// The corpus projection: a JSON array with one object per live document. This
/// is the intermediate the relational tables are materialized from.
pub fn corpus_json(pcfg: &ProjectConfig, docs: &[&Doc]) -> Value {
    Value::Array(docs.iter().map(|d| doc_json(pcfg, d)).collect())
}

/// A SQL single-quoted string literal (doubles embedded quotes).
fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// A SQL literal for an optional string: the quoted text, or `NULL`.
fn opt_lit(v: Option<&str>) -> String {
    v.map(sql_lit).unwrap_or_else(|| "NULL".to_string())
}

/// Stringify a scalar JSON value for a `fields`/text column (objects/arrays are
/// compacted to JSON text; that shape is rare for frontmatter scalars).
pub(crate) fn json_scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Build the `CREATE TABLE` + `INSERT` DDL/DML that materializes the corpus into
/// the four relational tables the stats SQL queries. Empty tables get no INSERT.
fn materialize(corpus: &Value) -> String {
    let mut docs_rows: Vec<String> = Vec::new();
    let mut tag_rows: Vec<String> = Vec::new();
    let mut section_rows: Vec<String> = Vec::new();
    let mut field_rows: Vec<String> = Vec::new();

    for d in corpus.as_array().into_iter().flatten() {
        let id = d["id"].as_str().unwrap_or("");
        let num = d["num"].as_i64().unwrap_or(0);
        docs_rows.push(format!(
            "({}, {}, {}, {}, {}, {}, {})",
            sql_lit(id),
            num,
            opt_lit(d["type"].as_str()),
            opt_lit(d["status"].as_str()),
            sql_lit(d["title"].as_str().unwrap_or("")),
            opt_lit(d["created"].as_str()),
            opt_lit(d["updated"].as_str()),
        ));

        for t in d["tags"].as_array().into_iter().flatten() {
            if let Some(tag) = t.as_str() {
                let (key, value) = split_tag(tag);
                tag_rows.push(format!(
                    "({}, {}, {}, {})",
                    sql_lit(id),
                    sql_lit(tag),
                    sql_lit(key),
                    opt_lit(value),
                ));
            }
        }

        if let Some(obj) = d["sections"].as_object() {
            for (heading, s) in obj {
                section_rows.push(format!(
                    "({}, {}, {}, {}, {})",
                    sql_lit(id),
                    sql_lit(heading),
                    opt_lit(s["kind"].as_str()),
                    s["items"].as_i64().unwrap_or(0),
                    s["unchecked"].as_i64().unwrap_or(0),
                ));
            }
        }

        if let Some(obj) = d["fields"].as_object() {
            for (key, v) in obj {
                // A list field contributes one row per element; a scalar, one row.
                match v {
                    Value::Array(items) => {
                        for it in items {
                            field_rows.push(format!(
                                "({}, {}, {})",
                                sql_lit(id),
                                sql_lit(key),
                                sql_lit(&json_scalar(it)),
                            ));
                        }
                    }
                    _ => field_rows.push(format!(
                        "({}, {}, {})",
                        sql_lit(id),
                        sql_lit(key),
                        sql_lit(&json_scalar(v)),
                    )),
                }
            }
        }
    }

    let mut sql = String::new();
    sql.push_str(
        "CREATE TABLE docs (id TEXT, num INTEGER, type TEXT, status TEXT, title TEXT, created TEXT, updated TEXT);\n",
    );
    sql.push_str("CREATE TABLE tags (doc_id TEXT, tag TEXT, key TEXT, value TEXT);\n");
    sql.push_str(
        "CREATE TABLE sections (doc_id TEXT, heading TEXT, kind TEXT, items INTEGER, unchecked INTEGER);\n",
    );
    sql.push_str("CREATE TABLE fields (doc_id TEXT, key TEXT, value TEXT);\n");

    let mut insert = |table: &str, rows: &[String]| {
        if !rows.is_empty() {
            sql.push_str(&format!(
                "INSERT INTO {table} VALUES {};\n",
                rows.join(", ")
            ));
        }
    };
    insert("docs", &docs_rows);
    insert("tags", &tag_rows);
    insert("sections", &section_rows);
    insert("fields", &field_rows);
    sql
}

/// Render one GlueSQL value as a markdown-table cell string.
pub(crate) fn cell(v: &GlueValue) -> String {
    match v {
        GlueValue::Str(s) => s.clone(),
        GlueValue::Bool(b) => b.to_string(),
        GlueValue::I8(n) => n.to_string(),
        GlueValue::I16(n) => n.to_string(),
        GlueValue::I32(n) => n.to_string(),
        GlueValue::I64(n) => n.to_string(),
        GlueValue::I128(n) => n.to_string(),
        GlueValue::U8(n) => n.to_string(),
        GlueValue::U16(n) => n.to_string(),
        GlueValue::U32(n) => n.to_string(),
        GlueValue::U64(n) => n.to_string(),
        GlueValue::U128(n) => n.to_string(),
        GlueValue::F32(x) => fmt_float(*x as f64),
        GlueValue::F64(x) => fmt_float(*x),
        GlueValue::Null => String::new(),
        other => format!("{other:?}"),
    }
}

/// Format a float without a trailing `.0` (so `ROUND(...)` reads as `67`, not
/// `67.0`), but keep real fractions.
fn fmt_float(x: f64) -> String {
    if x.is_finite() && x.fract() == 0.0 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

/// Escape a cell for a GFM table (pipes and newlines would break the row).
fn esc(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

/// An in-memory corpus database. Built once per `opys stats` / `verify` run and
/// reused across every stat — materializing (and re-parsing the INSERT DML) once
/// per stat would be needless O(stats × corpus) work.
pub type CorpusDb = Glue<MemoryStorage>;

/// Materialize `corpus` into a fresh in-memory database (the four corpus tables).
pub fn build_db(corpus: &Value) -> std::result::Result<CorpusDb, String> {
    let setup = materialize(corpus);
    let mut glue = Glue::new(MemoryStorage::default());
    block_on(glue.execute(&setup)).map_err(|e| format!("corpus build failed ({e})"))?;
    Ok(glue)
}

/// Run one stat's `sql` over an already-built corpus DB, returning the SELECT
/// result as (column labels, string rows). `Err` carries a human-readable
/// problem (a query that fails to run, or is not a SELECT).
fn run_sql(
    db: &mut CorpusDb,
    sql: &str,
) -> std::result::Result<(Vec<String>, Vec<Vec<String>>), String> {
    let payloads = block_on(db.execute(sql)).map_err(|e| format!("query failed ({e})"))?;
    let last = payloads
        .last()
        .ok_or_else(|| "query produced no result set".to_string())?;
    match last {
        Payload::Select { labels, rows } => {
            let rows = rows.iter().map(|r| r.iter().map(cell).collect()).collect();
            Ok((labels.clone(), rows))
        }
        other => Err(format!(
            "query must end in a SELECT (got {})",
            payload_kind(other)
        )),
    }
}

fn payload_kind(p: &Payload) -> &'static str {
    match p {
        Payload::Select { .. } | Payload::SelectMap(_) => "a projection",
        Payload::Insert(_) => "INSERT",
        Payload::Update(_) => "UPDATE",
        Payload::Delete(_) => "DELETE",
        _ => "a non-select statement",
    }
}

/// A markdown table (with an `## name` heading) for a stat's result set. An
/// empty result renders as a note rather than a malformed (header-only) table.
pub(crate) fn render_table(name: &str, labels: &[String], rows: &[Vec<String>]) -> String {
    format!("## {name}\n\n{}", table_body(labels, rows))
}

/// The bare markdown table (no heading) — shared with `opys query`.
pub(crate) fn table_body(labels: &[String], rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    if labels.is_empty() || rows.is_empty() {
        out.push_str("_(no rows)_\n");
        return out;
    }
    let header: Vec<String> = labels.iter().map(|l| esc(l)).collect();
    out.push_str(&format!("| {} |\n", header.join(" | ")));
    out.push_str(&format!("| {} |\n", vec!["---"; labels.len()].join(" | ")));
    for r in rows {
        let cells: Vec<String> = (0..labels.len())
            .map(|i| esc(r.get(i).map(String::as_str).unwrap_or("")))
            .collect();
        out.push_str(&format!("| {} |\n", cells.join(" | ")));
    }
    out
}

/// Print rendered markdown: styled for a terminal, raw when piped, `plain`,
/// or `NO_COLOR` — shared by `stats` and `query`.
pub(crate) fn print_markdown(out: &str, plain: bool) {
    let styled =
        !plain && std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    if styled {
        termimad::MadSkin::default().print_text(out);
    } else {
        println!("{out}");
    }
}

/// Expand a stat `template`: substitute `{column}` with `resolve(column)`,
/// treating `{{`/`}}` as literal braces. `Err` on an unclosed/unmatched brace or
/// a placeholder `resolve` rejects (an unknown column).
fn expand_template(
    tmpl: &str,
    resolve: &dyn Fn(&str) -> Option<String>,
) -> std::result::Result<String, String> {
    let mut out = String::new();
    let mut chars = tmpl.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut name = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(ch) => name.push(ch),
                        None => return Err("unclosed '{' in template".to_string()),
                    }
                }
                let key = name.trim();
                match resolve(key) {
                    Some(v) => out.push_str(&v),
                    None => return Err(format!("template references unknown column '{{{key}}}'")),
                }
            }
            '}' => return Err("unmatched '}' in template (use '}}' for a literal)".to_string()),
            _ => out.push(c),
        }
    }
    Ok(out)
}

/// Render a stat's result set through its row `template` (each row → one rendered
/// block, joined under the `## name` heading). Validates placeholders against the
/// column labels first, so an unknown `{column}` fails even with zero rows.
fn render_templated(
    name: &str,
    labels: &[String],
    rows: &[Vec<String>],
    tmpl: &str,
) -> std::result::Result<String, String> {
    let known = |k: &str| labels.iter().any(|l| l == k);
    // Column/brace check independent of row count (catches config errors early).
    expand_template(tmpl, &|k| known(k).then(String::new))?;

    let mut out = format!("## {name}\n\n");
    if rows.is_empty() {
        out.push_str("_(no rows)_\n");
        return Ok(out);
    }
    for row in rows {
        let cell_for = |k: &str| {
            labels
                .iter()
                .position(|l| l == k)
                .map(|i| row.get(i).cloned().unwrap_or_default())
        };
        out.push_str(&expand_template(tmpl, &cell_for)?);
        out.push('\n');
    }
    Ok(out)
}

/// Run one stat against an already-built corpus DB and format the result — as a
/// markdown table, or through the stat's row `template` when set. Returns a
/// human-readable problem (prefixed with the stat name) on failure. Used by
/// `render_all` and by `verify`.
pub fn render_stat_on(db: &mut CorpusDb, spec: &StatSpec) -> std::result::Result<String, String> {
    let (labels, rows) =
        run_sql(db, &spec.sql).map_err(|e| format!("stats '{}': {e}", spec.name))?;
    match &spec.template {
        Some(tmpl) => render_templated(&spec.name, &labels, &rows, tmpl)
            .map_err(|e| format!("stats '{}': {e}", spec.name)),
        None => Ok(render_table(&spec.name, &labels, &rows)),
    }
}

/// Render one stat over `corpus` (builds a DB just for it). Convenience for
/// config-time validation over an empty corpus.
pub fn render_stat(spec: &StatSpec, corpus: &Value) -> std::result::Result<String, String> {
    let mut db = build_db(corpus)?;
    render_stat_on(&mut db, spec)
}

/// Render every configured stat over `docs`, concatenated with a blank line
/// between sections. The corpus DB is built once and reused. `Err` carries the
/// first failing stat's problem message.
pub fn render_all(pcfg: &ProjectConfig, docs: &[&Doc]) -> std::result::Result<String, String> {
    let corpus = corpus_json(pcfg, docs);
    let mut db = build_db(&corpus)?;
    let mut sections = Vec::new();
    for spec in &pcfg.stats {
        sections.push(render_stat_on(&mut db, spec)?.trim_end().to_string());
    }
    Ok(sections.join("\n\n"))
}

pub fn run(ctx: &Ctx, plain: bool) -> Result<()> {
    let prj = ctx.open()?;
    let (docs, _) = prj.load_docs();
    let doc_refs: Vec<&Doc> = docs.iter().collect();
    let out = render_all(&prj.pcfg, &doc_refs).map_err(usage)?;
    if out.is_empty() {
        return Ok(());
    }
    print_markdown(&out, plain);
    Ok(())
}
