//! The in-memory SQL store — the internal working representation of the
//! inventory. Markdown files remain the durable storage; per CLI invocation
//! the corpus is loaded and decomposed into GlueSQL tables (`schema`), commands
//! operate on those tables (plus reconstructed [`Doc`]s for the Rust-side
//! invariants: rules, linkify, mdprism), and [`Store::flush`] writes changed
//! documents back to disk (create/rename/delete/ledger included).
//!
//! Internal SQL conventions: values are ALWAYS bound as `$n` parameters (never
//! string-interpolated); no `IN (subquery)`, no FROM-subqueries, no `UNION`
//! (unsupported), no correlated subqueries — GlueSQL executes those as O(n×m)
//! nested loops or rejects them. Multi-table aggregates run as separate simple
//! statements combined in Rust.

mod decompose;
mod flush;
mod model;
pub(crate) mod projection;
mod reconstruct;
mod schema;

use std::path::{Path, PathBuf};

use futures::executor::block_on;
use gluesql::core::error::Error as GlueError;
use gluesql::prelude::{Glue, MemoryStorage, ParamLiteral, Payload, Value as GValue};

use crate::doc::Doc;
use crate::error::{OpysError, Result};
use crate::project::Project;
use crate::project_config::ProjectConfig;

#[allow(unused_imports)] // used by command ports as they land
pub(crate) use decompose::split_tag;
pub(crate) use decompose::{decompose, id_num};
pub(crate) use model::Baseline;

/// The in-memory corpus database for one CLI invocation.
pub struct Store {
    glue: Glue<MemoryStorage>,
    /// Inventory base directory (absolute); `docs.path` is stored relative to it.
    base: PathBuf,
    /// Load snapshot `(dkey, absolute path)` — deletion detection at flush.
    loaded: Vec<(i64, PathBuf)>,
    /// Ledger row count at load; flush rewrites the ledger iff it grew.
    retired_loaded: usize,
    /// Whether a legacy plaintext `_retired.txt` was present at load, so flush
    /// migrates it to `_retired.md` even when no id was reserved this run.
    retired_legacy: bool,
    next_dkey: i64,
    next_rkey: i64,
}

/// An already-read, already-parsed corpus snapshot — the input to
/// [`Store::build`]. The storage backend produces this from the durable medium
/// (reading and parsing documents and the retired ledger); `build` turns it
/// into the working store with no filesystem access.
pub struct LoadedCorpus {
    /// Each parsed document paired with its original mtime (rfc3339), used to
    /// backfill missing timestamps during sync.
    pub docs: Vec<(Doc, Option<String>)>,
    /// Non-fatal parse errors (unparsable files were skipped).
    pub errors: Vec<String>,
    /// The retired-id ledger as `(id, title)` pairs.
    pub retired: Vec<(String, String)>,
    /// Whether a legacy `_retired.txt` was present (triggers migration on flush).
    pub retired_legacy: bool,
}

impl Store {
    /// Build the working store from an already-read, already-parsed
    /// [`LoadedCorpus`] — the medium-agnostic half of loading (no filesystem
    /// access). The storage backend reads the medium and calls this.
    pub fn build(prj: &Project, loaded: LoadedCorpus) -> Result<(Store, Vec<String>)> {
        let mut glue = Glue::new(MemoryStorage::default());
        block_on(glue.execute(schema::DDL)).map_err(store_err)?;
        let mut store = Store {
            glue,
            base: prj.base.clone(),
            loaded: Vec::new(),
            retired_loaded: 0,
            retired_legacy: loaded.retired_legacy,
            next_dkey: 1,
            next_rkey: 1,
        };

        let mut doc_rows: Vec<Vec<ParamLiteral>> = Vec::new();
        let mut tag_rows: Vec<Vec<ParamLiteral>> = Vec::new();
        let mut rel_rows: Vec<Vec<ParamLiteral>> = Vec::new();
        let mut fm_rows: Vec<Vec<ParamLiteral>> = Vec::new();
        for (doc, mtime) in &loaded.docs {
            let dkey = store.next_dkey;
            store.next_dkey += 1;
            let relpath = store.relpath_of(&doc.path)?;
            let rows = decompose(&prj.pcfg, doc)?;
            doc_rows.push(vec![
                dkey.into_param(),
                rows.id.clone().into_param(),
                rows.num.into_param(),
                rows.type_name.clone().into_param(),
                rows.status.clone().into_param(),
                rows.title.clone().into_param(),
                rows.created.clone().into_param(),
                rows.updated.clone().into_param(),
                rows.body.clone().into_param(),
                relpath.clone().into_param(),
                relpath.clone().into_param(), // orig_path
                doc.to_text().into_param(),   // orig_text
                mtime.clone().into_param(),
            ]);
            push_child_rows(dkey, &rows, &mut tag_rows, &mut rel_rows, &mut fm_rows);
            store.loaded.push((dkey, doc.path.clone()));
        }
        store.insert_batch("docs", 13, doc_rows)?;
        store.insert_batch("tags", 6, tag_rows)?;
        store.insert_batch("relations", 9, rel_rows)?;
        store.insert_batch("fm_fields", 6, fm_rows)?;

        let mut rows = Vec::with_capacity(loaded.retired.len());
        for (id, title) in &loaded.retired {
            let rkey = store.next_rkey;
            store.next_rkey += 1;
            rows.push(vec![
                rkey.into_param(),
                id.clone().into_param(),
                id_num(id).into_param(),
                title.clone().into_param(),
            ]);
        }
        store.retired_loaded = rows.len();
        store.insert_batch("retired", 4, rows)?;
        Ok((store, loaded.errors))
    }

    /// Load the corpus from the local filesystem: read and parse every document
    /// and the retired ledger, then [`build`](Store::build) the working store.
    pub fn open(prj: &Project) -> Result<(Store, Vec<String>)> {
        let (docs, errors) = prj.load_docs();
        let docs: Vec<(Doc, Option<String>)> = docs
            .into_iter()
            .map(|d| {
                let m = mtime_rfc3339(&d.path);
                (d, m)
            })
            .collect();
        let retired = crate::retired::read(&prj.base);
        let retired_legacy = crate::retired::legacy_path(&prj.base).exists();
        Store::build(
            prj,
            LoadedCorpus {
                docs,
                errors,
                retired,
                retired_legacy,
            },
        )
    }

    // ------------------------------------------------------------------
    // SQL primitives
    // ------------------------------------------------------------------

    fn run(&mut self, sql: &str, params: Vec<ParamLiteral>) -> Result<Vec<Payload>> {
        block_on(self.glue.execute_with_params(sql, params)).map_err(store_err)
    }

    /// Execute a non-SELECT internal statement.
    pub fn exec(&mut self, sql: &str, params: Vec<ParamLiteral>) -> Result<()> {
        self.run(sql, params).map(|_| ())
    }

    /// Execute an internal SELECT; returns (column labels, rows) of the last
    /// payload.
    pub fn select(
        &mut self,
        sql: &str,
        params: Vec<ParamLiteral>,
    ) -> Result<(Vec<String>, Vec<Vec<GValue>>)> {
        let payloads = self.run(sql, params)?;
        match payloads.into_iter().last() {
            Some(Payload::Select { labels, rows }) => Ok((labels, rows)),
            other => Err(OpysError::Store(format!(
                "expected a SELECT result, got {other:?}"
            ))),
        }
    }

    /// One-row-one-column SELECT convenience; `None` when no row or NULL.
    pub fn scalar(&mut self, sql: &str, params: Vec<ParamLiteral>) -> Result<Option<GValue>> {
        let (_, rows) = self.select(sql, params)?;
        Ok(rows
            .into_iter()
            .next()
            .and_then(|r| r.into_iter().next().filter(|v| !matches!(v, GValue::Null))))
    }

    /// Multi-row INSERT with parameter binding, chunked to bound statement size.
    fn insert_batch(
        &mut self,
        table: &str,
        ncols: usize,
        rows: Vec<Vec<ParamLiteral>>,
    ) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let chunk_rows = (600 / ncols).max(1);
        for chunk in rows.chunks(chunk_rows) {
            let mut sql = format!("INSERT INTO {table} VALUES ");
            let mut params: Vec<ParamLiteral> = Vec::with_capacity(chunk.len() * ncols);
            let mut n = 0usize;
            for (i, row) in chunk.iter().enumerate() {
                debug_assert_eq!(row.len(), ncols);
                if i > 0 {
                    sql.push_str(", ");
                }
                sql.push('(');
                for c in 0..ncols {
                    if c > 0 {
                        sql.push_str(", ");
                    }
                    n += 1;
                    sql.push_str(&format!("${n}"));
                }
                sql.push(')');
                params.extend(row.iter().cloned());
            }
            self.exec(&sql, params)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Doc-level API
    // ------------------------------------------------------------------

    /// The row key for an id — first match in load (path) order, mirroring
    /// `Project::find` on a corpus with duplicate ids. `None` if absent.
    pub fn dkey_opt(&mut self, id: &str) -> Result<Option<i64>> {
        Ok(self
            .scalar(
                "SELECT dkey FROM docs WHERE id = $1 ORDER BY dkey LIMIT 1",
                vec![id.into_param()],
            )?
            .as_ref()
            .and_then(g_i64))
    }

    /// The row key for an id, or a not-found error.
    pub fn dkey_of(&mut self, id: &str) -> Result<i64> {
        self.dkey_opt(id)?
            .ok_or_else(|| OpysError::NotFound { id: id.to_string() })
    }

    /// Whether the doc with `id` (first in load order) has an entry for
    /// `target` in relation `field`. `false` if the id is absent.
    pub fn has_relation(&mut self, id: &str, field: &str, target: &str) -> Result<bool> {
        let Some(dkey) = self.dkey_opt(id)? else {
            return Ok(false);
        };
        Ok(self
            .scalar(
                "SELECT 1 FROM relations WHERE dkey = $1 AND field = $2 AND ref_id = $3 LIMIT 1",
                vec![dkey.into_param(), field.into_param(), target.into_param()],
            )?
            .is_some())
    }

    /// Insert (`dkey = None`) or replace (`Some`) a document's rows. The doc's
    /// `path` must already be its intended location (absolute, under base).
    /// Replacement preserves the row's load snapshot (`orig_*`), so flush still
    /// diffs against the original file.
    pub fn put_doc(&mut self, pcfg: &ProjectConfig, dkey: Option<i64>, doc: &Doc) -> Result<i64> {
        let rows = decompose(pcfg, doc)?;
        let relpath = self.relpath_of(&doc.path)?;
        let dkey = match dkey {
            Some(k) => {
                self.exec(
                    "UPDATE docs SET id = $1, num = $2, type = $3, status = $4, title = $5, \
                     created = $6, updated = $7, body = $8, path = $9 WHERE dkey = $10",
                    vec![
                        rows.id.clone().into_param(),
                        rows.num.into_param(),
                        rows.type_name.clone().into_param(),
                        rows.status.clone().into_param(),
                        rows.title.clone().into_param(),
                        rows.created.clone().into_param(),
                        rows.updated.clone().into_param(),
                        rows.body.clone().into_param(),
                        relpath.into_param(),
                        k.into_param(),
                    ],
                )?;
                for table in ["tags", "relations", "fm_fields"] {
                    self.exec(
                        &format!("DELETE FROM {table} WHERE dkey = $1"),
                        vec![k.into_param()],
                    )?;
                }
                k
            }
            None => {
                let k = self.next_dkey;
                self.next_dkey += 1;
                self.exec(
                    "INSERT INTO docs VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, \
                     NULL, NULL, NULL)",
                    vec![
                        k.into_param(),
                        rows.id.clone().into_param(),
                        rows.num.into_param(),
                        rows.type_name.clone().into_param(),
                        rows.status.clone().into_param(),
                        rows.title.clone().into_param(),
                        rows.created.clone().into_param(),
                        rows.updated.clone().into_param(),
                        rows.body.clone().into_param(),
                        relpath.into_param(),
                    ],
                )?;
                k
            }
        };
        let (mut t, mut r, mut f) = (Vec::new(), Vec::new(), Vec::new());
        push_child_rows(dkey, &rows, &mut t, &mut r, &mut f);
        self.insert_batch("tags", 6, t)?;
        self.insert_batch("relations", 9, r)?;
        self.insert_batch("fm_fields", 6, f)?;
        Ok(dkey)
    }

    /// Remove a document's rows entirely (close/retire); flush deletes the file.
    pub fn delete_doc(&mut self, dkey: i64) -> Result<()> {
        for table in ["docs", "tags", "relations", "fm_fields"] {
            self.exec(
                &format!("DELETE FROM {table} WHERE dkey = $1"),
                vec![dkey.into_param()],
            )?;
        }
        Ok(())
    }

    /// Stamp the auto-maintained timestamps for a *user-initiated* edit: always
    /// refresh `updated`; set `created` only when the document has no `created`
    /// key at all (a wrong-typed value living in `fm_fields` blocks backfill,
    /// exactly like today's `contains_key` check). Housekeeping (sync,
    /// reconcile, strikes, cleanup) must never call this.
    pub fn touch(&mut self, dkey: i64, now: &str) -> Result<()> {
        // `updated` is overwritten wherever it lives.
        self.exec(
            "DELETE FROM fm_fields WHERE dkey = $1 AND key = 'updated'",
            vec![dkey.into_param()],
        )?;
        self.exec(
            "UPDATE docs SET updated = $1 WHERE dkey = $2",
            vec![now.into_param(), dkey.into_param()],
        )?;
        let has_created_col = self
            .scalar(
                "SELECT created FROM docs WHERE dkey = $1",
                vec![dkey.into_param()],
            )?
            .is_some();
        let has_created_fm = self
            .scalar(
                "SELECT COUNT(*) FROM fm_fields WHERE dkey = $1 AND key = 'created'",
                vec![dkey.into_param()],
            )?
            .as_ref()
            .and_then(g_i64)
            .unwrap_or(0)
            > 0;
        if !has_created_col && !has_created_fm {
            self.exec(
                "UPDATE docs SET created = $1 WHERE dkey = $2",
                vec![now.into_param(), dkey.into_param()],
            )?;
        }
        Ok(())
    }

    /// Overwrite `docs.path` directly (the relative form; flush performs the
    /// rename). Callers that already know the target path use this to avoid the
    /// read in [`Self::set_canonical_path`].
    pub fn set_path(&mut self, dkey: i64, relpath: &str) -> Result<()> {
        self.exec(
            "UPDATE docs SET path = $1 WHERE dkey = $2",
            vec![relpath.into_param(), dkey.into_param()],
        )
    }

    /// Point `docs.path` at the canonical layout path for the doc's id/status
    /// (no-op when either is missing, matching `save_doc`). Flush performs the
    /// actual rename.
    pub fn set_canonical_path(&mut self, pcfg: &ProjectConfig, dkey: i64) -> Result<()> {
        let (_, rows) = self.select(
            "SELECT id, status FROM docs WHERE dkey = $1",
            vec![dkey.into_param()],
        )?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(());
        };
        if let (Some(id), Some(status)) = (g_str(&row[0]), g_str(&row[1])) {
            let rel = pcfg.doc_relpath(&id, &status);
            self.exec(
                "UPDATE docs SET path = $1 WHERE dkey = $2",
                vec![path_str(&rel).into_param(), dkey.into_param()],
            )?;
        }
        Ok(())
    }

    /// Reserve a retired id with its last-known `title`; flush writes the ledger.
    pub fn retire_id(&mut self, id: &str, title: &str) -> Result<()> {
        let rkey = self.next_rkey;
        self.next_rkey += 1;
        self.exec(
            "INSERT INTO retired VALUES ($1, $2, $3, $4)",
            vec![
                rkey.into_param(),
                id.into_param(),
                id_num(id).into_param(),
                title.into_param(),
            ],
        )?;
        Ok(())
    }

    /// The set of live document ids (the resolution universe for
    /// `rules::evaluate` and reference targets).
    pub fn doc_ids(&mut self) -> Result<std::collections::HashSet<String>> {
        let (_, rows) = self.select("SELECT id FROM docs WHERE id IS NOT NULL", vec![])?;
        Ok(rows.into_iter().filter_map(|r| g_str(&r[0])).collect())
    }

    /// The title of the doc with `id` (first in load order), or `None`.
    pub fn title_of(&mut self, id: &str) -> Result<Option<String>> {
        Ok(self
            .scalar(
                "SELECT title FROM docs WHERE id = $1 ORDER BY dkey LIMIT 1",
                vec![id.into_param()],
            )?
            .as_ref()
            .and_then(g_str))
    }

    /// Highest numeric id across live docs, every relation-map entry (struck or
    /// not), and the retired ledger — the global monotonic sequence.
    pub fn max_doc_num(&mut self) -> Result<i64> {
        let mut max = 0i64;
        for sql in [
            "SELECT MAX(num) FROM docs",
            "SELECT MAX(ref_num) FROM relations",
            "SELECT MAX(num) FROM retired",
        ] {
            if let Some(n) = self.scalar(sql, vec![])?.as_ref().and_then(g_i64) {
                max = max.max(n);
            }
        }
        Ok(max)
    }

    /// Next id for a type prefix: one past the global max, zero-padded.
    pub fn next_id_for(&mut self, prefix: &str, pad: usize) -> Result<String> {
        let next = self.max_doc_num()? + 1;
        Ok(format!("{prefix}-{next:0pad$}"))
    }

    // ------------------------------------------------------------------

    /// A doc's absolute path relativized to the inventory base (the `path`
    /// column representation).
    fn relpath_of(&self, path: &Path) -> Result<String> {
        let rel = path.strip_prefix(&self.base).map_err(|_| {
            OpysError::Store(format!(
                "doc path {} is not under the inventory base {}",
                path.display(),
                self.base.display()
            ))
        })?;
        Ok(path_str(rel))
    }

    /// The absolute path for a `path` column value.
    pub(crate) fn abspath(&self, relpath: &str) -> PathBuf {
        self.base.join(relpath)
    }
}

/// Push a decomposed doc's child-table parameter rows.
fn push_child_rows(
    dkey: i64,
    rows: &decompose::DocRows,
    tags: &mut Vec<Vec<ParamLiteral>>,
    rels: &mut Vec<Vec<ParamLiteral>>,
    fms: &mut Vec<Vec<ParamLiteral>>,
) {
    let doc_id = rows.id.clone();
    for t in &rows.tags {
        tags.push(vec![
            dkey.into_param(),
            doc_id.clone().into_param(),
            t.seq.into_param(),
            t.tag.clone().into_param(),
            t.key.clone().into_param(),
            t.value.clone().into_param(),
        ]);
    }
    for r in &rows.relations {
        rels.push(vec![
            dkey.into_param(),
            doc_id.clone().into_param(),
            r.field.into_param(),
            r.seq.into_param(),
            r.ref_id.clone().into_param(),
            r.ref_num.into_param(),
            r.raw_value.clone().into_param(),
            r.title.clone().into_param(),
            r.struck.into_param(),
        ]);
    }
    for f in &rows.fm_fields {
        fms.push(vec![
            dkey.into_param(),
            doc_id.clone().into_param(),
            f.key.clone().into_param(),
            f.value_yaml.clone().into_param(),
            f.value.clone().into_param(),
            f.kind.into_param(),
        ]);
    }
}

/// Convert any `IntoParamLiteral` into a `ParamLiteral` (ergonomic shorthand;
/// `Option<T>` maps `None` → SQL NULL).
pub(crate) trait IntoParam {
    fn into_param(self) -> ParamLiteral;
}
impl<T: gluesql::core::translate::IntoParamLiteral> IntoParam for T {
    fn into_param(self) -> ParamLiteral {
        self.into_param_literal()
    }
}

fn store_err(e: GlueError) -> OpysError {
    OpysError::Store(e.to_string())
}

// ----------------------------------------------------------------------
// GValue decode helpers
// ----------------------------------------------------------------------

pub(crate) fn g_str(v: &GValue) -> Option<String> {
    match v {
        GValue::Str(s) => Some(s.clone()),
        _ => None,
    }
}

pub(crate) fn g_i64(v: &GValue) -> Option<i64> {
    match v {
        GValue::I64(n) => Some(*n),
        GValue::I32(n) => Some(i64::from(*n)),
        GValue::I16(n) => Some(i64::from(*n)),
        GValue::I8(n) => Some(i64::from(*n)),
        GValue::U64(n) => i64::try_from(*n).ok(),
        GValue::U32(n) => Some(i64::from(*n)),
        _ => None,
    }
}

#[allow(dead_code)] // used by command ports as they land
pub(crate) fn g_bool(v: &GValue) -> Option<bool> {
    match v {
        GValue::Bool(b) => Some(*b),
        _ => None,
    }
}

/// A path as a `path`-column string (forward-slash normalized display form).
fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A file's modification time as an RFC3339 datetime (second precision), or
/// `None` if unreadable. Captured at load for sync's timestamp backfill.
fn mtime_rfc3339(path: &Path) -> Option<String> {
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;
    let mt = std::fs::metadata(path).ok()?.modified().ok()?;
    let dt = OffsetDateTime::from(mt);
    let dt = dt.replace_nanosecond(0).unwrap_or(dt);
    dt.format(&Rfc3339).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CFG: &str = r#"
pad = 4
[types.feature]
prefix = "FEAT"
statuses = ["planned", "implemented", "archived"]
default_status = "planned"
status_dirs = { archived = "_archived" }
[types.feature.fields.priority]
type = "string"
[types.feature.fields.area]
type = "list"
[types.task]
prefix = "TASK"
statuses = ["todo", "done"]
default_status = "todo"
terminal_statuses = ["done"]
"#;

    /// A temp project: opys.toml + base dir with the given (relpath, text) docs.
    fn project_with(docs: &[(&str, &str)]) -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("opys.toml"), CFG).unwrap();
        let base = dir.path().join("opys");
        std::fs::create_dir_all(&base).unwrap();
        for (rel, text) in docs {
            let p = base.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let prj = Project::open(&dir.path().to_string_lossy()).expect("open project");
        (dir, prj)
    }

    /// Every GlueSQL construct the store (and the future sync engine) relies
    /// on, probed in one place so a gluesql upgrade failing any of them is
    /// caught here, not in a command.
    #[test]
    fn glue_probes() {
        let (_t, prj) = project_with(&[]);
        let (mut s, errs) = Store::open(&prj).unwrap();
        assert!(errs.is_empty());

        // MAX over an empty table is NULL → scalar() → None.
        assert!(s
            .scalar("SELECT MAX(num) FROM docs", vec![])
            .unwrap()
            .is_none());

        // Parameterized INSERT / SELECT / UPDATE / DELETE round-trip.
        s.exec(
            "INSERT INTO retired VALUES ($1, $2, $3, $4)",
            vec![
                1i64.into_param(),
                "FEAT-0002".into_param(),
                2i64.into_param(),
                "FEAT-0002  # retired".into_param(),
            ],
        )
        .unwrap();
        let v = s
            .scalar(
                "SELECT num FROM retired WHERE id = $1 LIMIT 1",
                vec!["FEAT-0002".into_param()],
            )
            .unwrap();
        assert_eq!(v.as_ref().and_then(g_i64), Some(2));
        s.exec(
            "UPDATE retired SET title = $1 WHERE id = $2",
            vec!["x".into_param(), "FEAT-0002".into_param()],
        )
        .unwrap();
        s.exec(
            "DELETE FROM retired WHERE id = $1",
            vec!["FEAT-0002".into_param()],
        )
        .unwrap();

        // COUNT(DISTINCT …), COALESCE, boolean params.
        s.exec(
            "INSERT INTO relations VALUES \
             ($1,$2,$3,$4,$5,$6,$7,$8,$9), ($10,$11,$12,$13,$14,$15,$16,$17,$18)",
            vec![
                1i64.into_param(),
                "A-1".into_param(),
                "references".into_param(),
                0i64.into_param(),
                "B-2".into_param(),
                2i64.into_param(),
                "T".into_param(),
                "T".into_param(),
                false.into_param(),
                1i64.into_param(),
                "A-1".into_param(),
                "blocks".into_param(),
                1i64.into_param(),
                "B-2".into_param(),
                2i64.into_param(),
                "~~T~~".into_param(),
                "T".into_param(),
                true.into_param(),
            ],
        )
        .unwrap();
        let n = s
            .scalar("SELECT COUNT(DISTINCT ref_id) FROM relations", vec![])
            .unwrap();
        assert_eq!(n.as_ref().and_then(g_i64), Some(1));
        // Pin a GlueSQL 0.19 gap: COALESCE over an aggregate does NOT rescue
        // the empty-table NULL. Internal code must default in Rust (as
        // `max_doc_num` does), never via COALESCE(MAX(…), n).
        let c = s
            .scalar("SELECT COALESCE(MAX(num), 0) FROM docs", vec![])
            .unwrap();
        assert_eq!(c.as_ref().and_then(g_i64), None);
        let struck = s
            .scalar(
                "SELECT COUNT(*) FROM relations WHERE struck = $1",
                vec![true.into_param()],
            )
            .unwrap();
        assert_eq!(struck.as_ref().and_then(g_i64), Some(1));

        // Multi-condition LEFT JOIN … ON a AND b, with IS NULL filtering — the
        // shape the sync engine's missing-inverse-edge queries use.
        let (_, rows) = s
            .select(
                "SELECT r.ref_id FROM relations r \
                 LEFT JOIN relations r2 ON r2.field = r.field AND r2.ref_id = 'nope' \
                 WHERE r2.ref_id IS NULL ORDER BY r.field",
                vec![],
            )
            .unwrap();
        assert_eq!(rows.len(), 2);
    }

    /// The gate invariant: decompose → insert → reconstruct → to_text is
    /// byte-identical for every fidelity edge shape.
    #[test]
    fn fixpoint_over_adversarial_docs() {
        let docs: &[(&str, &str)] = &[
            // Plain canonical doc with tags, relations (incl. tombstone), fields.
            ("FEAT-0001.md",
             "---\nid: FEAT-0001\nstatus: planned\ntags: [osc, area:parsing]\n\
              created: \"2026-01-01T00:00:00Z\"\npriority: high\nreferences:\n  \
              TASK-0002: ~~Closed thing~~\n  TASK-0003: Live thing\n---\n\n# One\n\nBody prose.\n"),
            // Wrong-typed core fields + empty tags + empty relation map.
            ("FEAT-0004.md",
             "---\nid: FEAT-0004\nstatus: 5\ntags: []\nreferences: {}\n---\n\n# Four\n"),
            // Nested custom field, block scalar, non-string relation value.
            ("FEAT-0005.md",
             "---\nid: FEAT-0005\nstatus: planned\nmeta:\n  nested:\n    deep: [1, 2]\n\
              note: |\n  line one\n  line two\nblocked_by:\n  TASK-0002: 7\n---\n\n# Five\n"),
            // Hand-edited (unsorted) relation map order must round-trip.
            ("FEAT-0006.md",
             "---\nid: FEAT-0006\nstatus: planned\nreferences:\n  TASK-0009: Nine\n  \
              TASK-0002: Two\n---\n\n# Six\n"),
            // Missing id; string-that-looks-like-a-number; list field.
            ("stray/FEAT-0007.md",
             "---\nid: FEAT-0007\nstatus: planned\npriority: \"5\"\narea: [ui, core]\n---\n\n# Seven\n"),
        ];
        let (_t, prj) = project_with(docs);
        let (mut s, errs) = Store::open(&prj).unwrap();
        assert!(errs.is_empty(), "parse errors: {errs:?}");

        let (orig, _) = prj.load_docs();
        let rebuilt = s.all_docs().unwrap();
        assert_eq!(orig.len(), rebuilt.len());
        for (o, (_, r)) in orig.iter().zip(&rebuilt) {
            assert_eq!(
                o.to_text(),
                r.to_text(),
                "fixpoint failed for {}",
                o.path.display()
            );
            assert_eq!(o.path, r.path);
        }
    }

    /// Flush right after open must not modify a single byte on disk.
    #[test]
    fn open_then_flush_is_a_noop() {
        // Note the second file is deliberately NON-canonical (odd key order,
        // quoted plain scalar): flush must leave it untouched because the doc
        // did not logically change.
        let docs: &[(&str, &str)] = &[
            (
                "FEAT-0001.md",
                "---\nid: FEAT-0001\nstatus: planned\n---\n\n# A\n",
            ),
            (
                "FEAT-0002.md",
                "---\nstatus: planned\nid: \"FEAT-0002\"\n---\n# B\n",
            ),
        ];
        let (_t, prj) = project_with(docs);
        let before: Vec<String> = docs
            .iter()
            .map(|(rel, _)| std::fs::read_to_string(prj.base.join(rel)).unwrap())
            .collect();
        let (s, _) = Store::open(&prj).unwrap();
        s.flush(&prj).unwrap();
        for ((rel, _), b) in docs.iter().zip(&before) {
            let after = std::fs::read_to_string(prj.base.join(rel)).unwrap();
            assert_eq!(*b, after, "{rel} changed on a no-op flush");
        }
    }

    /// put_doc + set_canonical_path relocates on flush; delete_doc removes the
    /// file; retire_id rewrites the ledger sorted by number.
    #[test]
    fn flush_applies_writes_renames_deletes_and_ledger() {
        let docs: &[(&str, &str)] = &[
            (
                "FEAT-0001.md",
                "---\nid: FEAT-0001\nstatus: planned\n---\n\n# A\n",
            ),
            (
                "FEAT-0002.md",
                "---\nid: FEAT-0002\nstatus: planned\n---\n\n# B\n",
            ),
            (
                "FEAT-0003.md",
                "---\nid: FEAT-0003\nstatus: planned\n---\n\n# C\n",
            ),
        ];
        let (_t, prj) = project_with(docs);
        std::fs::write(
            prj.base.join("_retired.txt"),
            "FEAT-0009  # retired 2026-01-01: old\n",
        )
        .unwrap();
        let (mut s, _) = Store::open(&prj).unwrap();

        // Archive FEAT-0001 → moves into _archived/.
        let k1 = s.dkey_of("FEAT-0001").unwrap();
        let mut d1 = s.doc(k1).unwrap();
        d1.frontmatter.set_str("status", "archived");
        s.put_doc(&prj.pcfg, Some(k1), &d1).unwrap();
        s.set_canonical_path(&prj.pcfg, k1).unwrap();

        // Delete FEAT-0002 outright; retire FEAT-0003 (delete + ledger).
        let k2 = s.dkey_of("FEAT-0002").unwrap();
        s.delete_doc(k2).unwrap();
        let k3 = s.dkey_of("FEAT-0003").unwrap();
        s.retire_id("FEAT-0003", "C").unwrap();
        s.delete_doc(k3).unwrap();

        s.flush(&prj).unwrap();

        assert!(!prj.base.join("FEAT-0001.md").exists());
        let archived = prj.base.join("_archived/FEAT-0001.md");
        assert!(archived.exists(), "archived doc not relocated");
        assert!(std::fs::read_to_string(&archived)
            .unwrap()
            .contains("status: archived"));
        assert!(!prj.base.join("FEAT-0002.md").exists());
        assert!(!prj.base.join("FEAT-0003.md").exists());
        // The legacy plaintext ledger was migrated to `_retired.md`, keeping
        // both the newly retired id (with its title) and the pre-seeded one.
        assert!(!prj.base.join("_retired.txt").exists());
        let ledger = std::fs::read_to_string(prj.base.join("_retired.md")).unwrap();
        assert!(ledger.contains("FEAT-0003: C"), "ledger = {ledger}");
        assert!(ledger.contains("FEAT-0009"), "ledger = {ledger}");
    }

    /// `updated` always bumps; `created` backfills only when the key is absent
    /// everywhere — a wrong-typed `created` (in fm_fields) blocks the backfill.
    #[test]
    fn touch_bumps_updated_and_respects_existing_created() {
        let docs: &[(&str, &str)] = &[
            (
                "FEAT-0001.md",
                "---\nid: FEAT-0001\nstatus: planned\n---\n\n# A\n",
            ),
            (
                "FEAT-0002.md",
                "---\nid: FEAT-0002\nstatus: planned\ncreated: 7\nupdated: 8\n---\n\n# B\n",
            ),
        ];
        let (_t, prj) = project_with(docs);
        let (mut s, _) = Store::open(&prj).unwrap();

        let k1 = s.dkey_of("FEAT-0001").unwrap();
        s.touch(k1, "2026-03-03T00:00:00Z").unwrap();
        let d1 = s.doc(k1).unwrap();
        assert_eq!(
            d1.frontmatter.get_str("created"),
            Some("2026-03-03T00:00:00Z")
        );
        assert_eq!(
            d1.frontmatter.get_str("updated"),
            Some("2026-03-03T00:00:00Z")
        );

        // Wrong-typed created (int) must survive; wrong-typed updated is replaced.
        let k2 = s.dkey_of("FEAT-0002").unwrap();
        s.touch(k2, "2026-03-03T00:00:00Z").unwrap();
        let d2 = s.doc(k2).unwrap();
        assert_eq!(
            d2.frontmatter.get("created"),
            Some(&serde_norway::Value::Number(7.into()))
        );
        assert_eq!(
            d2.frontmatter.get_str("updated"),
            Some("2026-03-03T00:00:00Z")
        );
    }

    /// The id sequence covers live docs, relation-map ids (struck or not), and
    /// the retired ledger.
    #[test]
    fn next_id_covers_docs_relations_and_ledger() {
        let docs: &[(&str, &str)] = &[(
            "FEAT-0001.md",
            "---\nid: FEAT-0001\nstatus: planned\nreferences:\n  TASK-0007: ~~Gone~~\n---\n\n# A\n",
        )];
        let (_t, prj) = project_with(docs);
        std::fs::write(prj.base.join("_retired.txt"), "FEAT-0012  # retired\n").unwrap();
        let (mut s, _) = Store::open(&prj).unwrap();
        assert_eq!(s.next_id_for("FEAT", 4).unwrap(), "FEAT-0013");
    }

    /// User SQL is statement-guarded on the plan: no mutation can execute, even
    /// ahead of a trailing SELECT.
    #[test]
    fn user_query_rejects_non_select() {
        let docs: &[(&str, &str)] = &[(
            "FEAT-0001.md",
            "---\nid: FEAT-0001\nstatus: planned\n---\n\n# A\n",
        )];
        let (_t, prj) = project_with(docs);
        let (mut s, _) = Store::open(&prj).unwrap();

        for bad in [
            "DELETE FROM docs",
            "DROP TABLE docs",
            "INSERT INTO docs SELECT * FROM docs",
            "UPDATE docs SET status = 'x'",
            "DELETE FROM docs; SELECT 1",
        ] {
            let err = s.run_user_query(bad).unwrap_err();
            assert!(
                err.contains("must be a SELECT") || err.contains("query failed"),
                "{bad}: {err}"
            );
        }
        // Nothing executed: the doc row is intact.
        let n = s.scalar("SELECT COUNT(*) FROM docs", vec![]).unwrap();
        assert_eq!(n.as_ref().and_then(g_i64), Some(1));

        let (labels, rows) = s
            .run_user_query("SELECT id, status FROM docs ORDER BY id")
            .unwrap();
        assert_eq!(labels, vec!["id", "status"]);
        assert_eq!(
            rows,
            vec![vec!["FEAT-0001".to_string(), "planned".to_string()]]
        );
    }
}
