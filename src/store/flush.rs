//! Persisting the store back to the durable medium is split in two: this module
//! computes a **medium-agnostic** [`FlushPlan`] from the store's rows and load
//! snapshot (no filesystem access), and the storage backend executes it. The
//! plan makes the medium match the tables: docs deleted from the store lose
//! their files, docs whose `path` moved are renamed, and a doc's file is
//! (re)written only when its reconstructed canonical text differs from the text
//! rendered at load. Ordering on apply is deletes → renames → writes, so a
//! rename never lands on a path a delete is about to vacate. Finally
//! `_retired.md` is (re)written iff an id was reserved this run, and any legacy
//! `_retired.txt` is migrated to it.
//!
//! [`apply_plan`] is the local-filesystem executor, kept here so the pure plan
//! and its one in-tree consumer stay together; a backend for another medium
//! would execute the same plan differently.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::project::Project;

use super::{g_i64, g_str, Store};

/// What a flush must persist, as absolute paths and rendered text — computed
/// without touching the filesystem so any backend can execute it.
#[derive(Debug, Default)]
pub struct FlushPlan {
    /// Document files to remove (a doc was deleted/closed/retired).
    pub deletes: Vec<PathBuf>,
    /// Document files to move (`from`, `to`) after a layout/status change.
    pub renames: Vec<(PathBuf, PathBuf)>,
    /// Document files to (re)write (`path`, canonical text).
    pub writes: Vec<(PathBuf, String)>,
    /// The retired ledger to (re)write (`_retired.md` path, serialized text).
    pub retired_write: Option<(PathBuf, String)>,
    /// A legacy `_retired.txt` to delete (migrated to `_retired.md`).
    pub legacy_remove: Option<PathBuf>,
}

impl Store {
    /// Compute the [`FlushPlan`] — no filesystem access. Consumes the store, the
    /// final read of an invocation.
    pub fn flush_plan(mut self, prj: &Project) -> Result<FlushPlan> {
        let rows = self.full_rows()?;
        let mut plan = FlushPlan::default();

        // 1. Deletions: loaded rows that no longer exist in `docs`.
        let surviving: HashSet<i64> = rows.iter().map(|r| r.dkey).collect();
        for (dkey, load_path) in &self.loaded {
            if !surviving.contains(dkey) {
                plan.deletes.push(load_path.clone());
            }
        }

        // 2. Renames: the authoritative path moved away from the load path.
        for r in &rows {
            if let Some(orig) = &r.orig_relpath {
                if *orig != r.relpath {
                    plan.renames
                        .push((self.abspath(orig), self.abspath(&r.relpath)));
                }
            }
        }

        // 3. Writes: new docs always; existing docs only on logical change.
        for r in &rows {
            let text = r.doc.to_text();
            let write = match &r.orig_text {
                None => true,
                Some(orig) => *orig != text,
            };
            if write {
                plan.writes.push((self.abspath(&r.relpath), text));
            }
        }

        // 4. Retired ledger: (re)write `_retired.md` when an id was reserved
        //    this run or a legacy `_retired.txt` is present (migration). The
        //    legacy-present flag was captured at load, so this stays fs-free.
        let (_, retired) = self.select(
            "SELECT rkey, id, num, title FROM retired ORDER BY rkey",
            vec![],
        )?;
        let grew = retired.len() > self.retired_loaded;
        if grew || self.retired_legacy {
            let mut entries: Vec<(u64, (String, String))> = retired
                .iter()
                .map(|r| {
                    let num = g_i64(&r[2]).map(|n| n as u64).unwrap_or(u64::MAX);
                    let id = g_str(&r[1]).unwrap_or_default();
                    let title = g_str(&r[3]).unwrap_or_default();
                    (num, (id, title))
                })
                .collect();
            entries.sort_by_key(|e| e.0); // stable: ties keep rkey order
            let pairs: Vec<(String, String)> = entries.into_iter().map(|e| e.1).collect();
            if !pairs.is_empty() {
                plan.retired_write = Some((
                    crate::retired::path(&prj.base),
                    crate::retired::serialize(&pairs),
                ));
            }
            if self.retired_legacy {
                plan.legacy_remove = Some(crate::retired::legacy_path(&prj.base));
            }
        }
        Ok(plan)
    }

    /// Compute and apply the flush plan against the local filesystem. Consumes
    /// the store.
    pub fn flush(self, prj: &Project) -> Result<()> {
        let plan = self.flush_plan(prj)?;
        apply_plan(&plan, &prj.base)
    }
}

/// Execute a [`FlushPlan`] against the local filesystem: deletes → renames →
/// writes → ledger, pruning emptied document directories (never the base).
pub fn apply_plan(plan: &FlushPlan, base: &Path) -> Result<()> {
    for p in &plan.deletes {
        if p.exists() {
            std::fs::remove_file(p)?;
        }
        prune_empty_dir(p.parent(), base);
    }
    for (from, to) in &plan.renames {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if from.exists() {
            std::fs::rename(from, to)?;
            prune_empty_dir(from.parent(), base);
        }
    }
    for (target, text) in &plan.writes {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(target, text)?;
    }
    if let Some((path, text)) = &plan.retired_write {
        std::fs::write(path, text)?;
    }
    if let Some(legacy) = &plan.legacy_remove {
        std::fs::remove_file(legacy)?;
    }
    Ok(())
}

/// Best-effort removal of an emptied document directory (never the base).
fn prune_empty_dir(dir: Option<&Path>, base: &Path) {
    if let Some(dir) = dir {
        if dir != base && dir.starts_with(base) {
            let _ = std::fs::remove_dir(dir); // no-op unless empty
        }
    }
}
