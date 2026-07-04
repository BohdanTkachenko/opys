//! Write the store back to disk. The filesystem is made to match the tables:
//! docs deleted from the store lose their files, docs whose `path` moved are
//! renamed (dirs created, emptied source dirs pruned — never the base), and a
//! doc's file is (re)written only when its reconstructed canonical text differs
//! from the text rendered at load — so a hand-written non-canonical file is
//! only canonicalized when it logically changes (today's sync semantics).
//! Ordering is deletes → renames → writes, so a rename never lands on a path a
//! delete is about to vacate. Finally `_retired.md` is (re)written (reserved
//! id -> title, sorted by number) iff an id was reserved this run, and any
//! legacy `_retired.txt` is migrated to it.

use std::collections::HashSet;
use std::path::Path;

use crate::error::Result;
use crate::project::Project;

use super::{g_i64, g_str, Store};

impl Store {
    /// Flush all changes to disk. Consumes the store — flush is the final act
    /// of an invocation (run it even when the command failed: with the
    /// guards-before-mutation convention the store then holds exactly the
    /// successful sub-operations, matching today's per-id durability).
    pub fn flush(mut self, prj: &Project) -> Result<()> {
        let rows = self.full_rows()?;

        // 1. Deletions: loaded rows that no longer exist in `docs`.
        let surviving: HashSet<i64> = rows.iter().map(|r| r.dkey).collect();
        for (dkey, load_path) in &self.loaded {
            if !surviving.contains(dkey) {
                if load_path.exists() {
                    std::fs::remove_file(load_path)?;
                }
                prune_empty_dir(load_path.parent(), &prj.base);
            }
        }

        // 2. Renames: the authoritative path moved away from the load path.
        for r in &rows {
            if let Some(orig) = &r.orig_relpath {
                if *orig != r.relpath {
                    let from = self.abspath(orig);
                    let to = self.abspath(&r.relpath);
                    if let Some(parent) = to.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    if from.exists() {
                        std::fs::rename(&from, &to)?;
                        prune_empty_dir(from.parent(), &prj.base);
                    }
                }
            }
        }

        // 3. Writes: new docs always; existing docs only on logical change.
        for r in &rows {
            let target = self.abspath(&r.relpath);
            let text = r.doc.to_text();
            let write = match &r.orig_text {
                None => true,
                Some(orig) => *orig != text,
            };
            if write {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&target, text)?;
            }
        }

        // 4. Retired ledger (`crate::retired`): write `_retired.md` (reserved
        //    id -> title, sorted by number) when an id was reserved this run or
        //    a legacy plaintext `_retired.txt` needs migrating, then drop the
        //    legacy file.
        let (_, retired) = self.select(
            "SELECT rkey, id, num, title FROM retired ORDER BY rkey",
            vec![],
        )?;
        let grew = retired.len() > self.retired_loaded;
        let legacy = crate::retired::legacy_path(&prj.base);
        let legacy_exists = legacy.exists();
        if grew || legacy_exists {
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
                std::fs::write(
                    crate::retired::path(&prj.base),
                    crate::retired::serialize(&pairs),
                )?;
            }
            if legacy_exists {
                std::fs::remove_file(&legacy)?;
            }
        }
        Ok(())
    }
}

/// Best-effort removal of an emptied document directory (never the base).
fn prune_empty_dir(dir: Option<&Path>, base: &Path) {
    if let Some(dir) = dir {
        if dir != base && dir.starts_with(base) {
            let _ = std::fs::remove_dir(dir); // no-op unless empty
        }
    }
}
