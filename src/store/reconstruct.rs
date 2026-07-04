//! Rows → Doc. Rebuilds the exact `Frontmatter` mapping from the partitioned
//! tables (columns + tags + relations + fm_fields); the canonical serializer
//! makes key insertion order irrelevant, so the merge is order-free. The gate
//! invariant is `reconstruct(decompose(doc)).to_text() == doc.to_text()`.

use std::collections::BTreeMap;

use serde_norway::{Mapping, Value as Yaml};

use crate::body;
use crate::doc::Doc;
use crate::error::{OpysError, Result};
use crate::frontmatter::Frontmatter;

use super::{g_i64, g_str, IntoParam, Store};

/// One fully-reconstructed row of `docs` plus its load snapshot (flush input).
pub(crate) struct FullRow {
    pub dkey: i64,
    pub relpath: String,
    pub orig_relpath: Option<String>,
    pub orig_text: Option<String>,
    #[allow(dead_code)] // sync's mtime backfill (engine flip step)
    pub orig_mtime: Option<String>,
    pub doc: Doc,
}

/// Partial frontmatter parts collected per dkey before assembly.
#[derive(Default)]
struct Parts {
    tags: Vec<(i64, String)>,
    relations: Vec<(String, i64, String, String)>, // (field, seq, ref_id, raw_value)
    fm_fields: Vec<(String, String)>,              // (key, value_yaml)
}

impl Store {
    /// Reconstruct a single document.
    pub fn doc(&mut self, dkey: i64) -> Result<Doc> {
        let (_, rows) = self.select(
            "SELECT dkey, id, status, created, updated, body, path FROM docs WHERE dkey = $1",
            vec![dkey.into_param()],
        )?;
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| OpysError::Store(format!("no docs row for dkey {dkey}")))?;
        let parts = self
            .parts_for(Some(dkey))?
            .remove(&dkey)
            .unwrap_or_default();
        self.assemble(&row, parts).map(|f| f.doc)
    }

    /// Reconstruct every document, in dkey (load/path) order.
    pub fn all_docs(&mut self) -> Result<Vec<(i64, Doc)>> {
        Ok(self
            .full_rows()?
            .into_iter()
            .map(|f| (f.dkey, f.doc))
            .collect())
    }

    /// Every doc with its snapshot columns — the flush working set.
    pub(crate) fn full_rows(&mut self) -> Result<Vec<FullRow>> {
        let (_, rows) = self.select(
            "SELECT dkey, id, status, created, updated, body, path, \
             orig_path, orig_text, orig_mtime FROM docs ORDER BY dkey",
            vec![],
        )?;
        let mut parts = self.parts_for(None)?;
        rows.into_iter()
            .map(|row| {
                let dkey = g_i64(&row[0])
                    .ok_or_else(|| OpysError::Store("docs.dkey is not an integer".into()))?;
                let p = parts.remove(&dkey).unwrap_or_default();
                self.assemble(&row, p)
            })
            .collect()
    }

    /// Child-table parts, for one dkey or all.
    fn parts_for(&mut self, dkey: Option<i64>) -> Result<BTreeMap<i64, Parts>> {
        let mut out: BTreeMap<i64, Parts> = BTreeMap::new();
        let (filter, params) = match dkey {
            Some(k) => (" WHERE dkey = $1", vec![k.into_param()]),
            None => ("", vec![]),
        };

        let (_, rows) = self.select(
            &format!("SELECT dkey, seq, tag FROM tags{filter} ORDER BY dkey, seq"),
            params.clone(),
        )?;
        for r in rows {
            let (k, seq) = (need_i64(&r[0])?, need_i64(&r[1])?);
            out.entry(k).or_default().tags.push((seq, need_str(&r[2])?));
        }

        let (_, rows) = self.select(
            &format!(
                "SELECT dkey, field, seq, ref_id, raw_value FROM relations{filter} \
                 ORDER BY dkey, seq"
            ),
            params.clone(),
        )?;
        for r in rows {
            let k = need_i64(&r[0])?;
            out.entry(k).or_default().relations.push((
                need_str(&r[1])?,
                need_i64(&r[2])?,
                need_str(&r[3])?,
                need_str(&r[4])?,
            ));
        }

        // Element rows (value_yaml NULL) are query-only; only fidelity rows
        // participate in reconstruction.
        let not_elem = if filter.is_empty() {
            " WHERE value_yaml IS NOT NULL"
        } else {
            " AND value_yaml IS NOT NULL"
        };
        let (_, rows) = self.select(
            &format!("SELECT dkey, key, value_yaml FROM fm_fields{filter}{not_elem} ORDER BY dkey"),
            params,
        )?;
        for r in rows {
            let k = need_i64(&r[0])?;
            out.entry(k)
                .or_default()
                .fm_fields
                .push((need_str(&r[1])?, need_str(&r[2])?));
        }
        Ok(out)
    }

    /// Assemble a docs row (dkey, id, status, created, updated, body, path,
    /// [orig_path, orig_text, orig_mtime]) + child parts into a `FullRow`.
    fn assemble(&self, row: &[gluesql::prelude::Value], parts: Parts) -> Result<FullRow> {
        let dkey =
            g_i64(&row[0]).ok_or_else(|| OpysError::Store("docs.dkey not integer".into()))?;
        let mut map = Mapping::new();
        for (i, key) in [(1, "id"), (2, "status"), (3, "created"), (4, "updated")] {
            if let Some(s) = g_str(&row[i]) {
                map.insert(Yaml::String(key.to_string()), Yaml::String(s));
            }
        }
        if !parts.tags.is_empty() {
            let mut tags = parts.tags;
            tags.sort_by_key(|(seq, _)| *seq);
            let seq = tags.into_iter().map(|(_, t)| Yaml::String(t)).collect();
            map.insert(Yaml::String("tags".to_string()), Yaml::Sequence(seq));
        }
        for field in crate::refs::RELATION_FIELDS {
            let mut entries: Vec<_> = parts.relations.iter().filter(|e| e.0 == field).collect();
            if entries.is_empty() {
                continue;
            }
            entries.sort_by_key(|e| e.1);
            let mut m = Mapping::new();
            for e in entries {
                m.insert(Yaml::String(e.2.clone()), Yaml::String(e.3.clone()));
            }
            map.insert(Yaml::String(field.to_string()), Yaml::Mapping(m));
        }
        for (key, value_yaml) in parts.fm_fields {
            let value: Yaml = serde_norway::from_str(&value_yaml)
                .map_err(|e| OpysError::Store(format!("cannot parse stored field '{key}': {e}")))?;
            map.insert(Yaml::String(key), value);
        }

        let body = need_str(&row[5])?;
        let relpath = need_str(&row[6])?;
        let title = body::title(&body);
        Ok(FullRow {
            dkey,
            orig_relpath: row.get(7).and_then(g_str),
            orig_text: row.get(8).and_then(g_str),
            orig_mtime: row.get(9).and_then(g_str),
            doc: Doc {
                path: self.abspath(&relpath),
                frontmatter: Frontmatter { map },
                body,
                title,
            },
            relpath,
        })
    }
}

fn need_str(v: &gluesql::prelude::Value) -> Result<String> {
    g_str(v).ok_or_else(|| OpysError::Store(format!("expected TEXT value, got {v:?}")))
}

fn need_i64(v: &gluesql::prelude::Value) -> Result<i64> {
    g_i64(v).ok_or_else(|| OpysError::Store(format!("expected INTEGER value, got {v:?}")))
}
