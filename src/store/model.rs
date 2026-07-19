//! The reactive model pass: after user `--write` DML mutates the raw tables,
//! react to the resulting *state delta* and materialize lifecycle cascades
//! before the corpus is validated and flushed. The pass reads STATE, never the
//! SQL text — so it is independent of how an edit was expressed (parsing SQL
//! shapes is unreliable; a document either exists afterwards or it doesn't).
//!
//! Today the one cascade is **removal**: a `DELETE` that drops a `docs` row is
//! treated as a lifecycle close — the id is reserved against reuse and every
//! inbound reference to it is struck into a `~~title~~` tombstone, so no live
//! reference is left dangling. The model pass and the validator (the `verify`
//! gate in `commands/query.rs`) are deliberately separate seams.

use crate::error::Result;
use crate::project_config::ProjectConfig;
use crate::refs;

use super::{g_i64, g_str, IntoParam, Store};

/// A load-time snapshot of `id -> (dkey, title)` for every live document, taken
/// before a `--write` runs so the model pass can diff against it afterwards.
pub type Baseline = Vec<(String, i64, String)>;

impl Store {
    /// Snapshot `id -> (dkey, title)` for every live document. Captured before
    /// user DML so [`Store::cascade_removals`] can find what the edit removed
    /// (and recover a removed doc's last-known title for its tombstone).
    pub fn baseline(&mut self) -> Result<Baseline> {
        let (_, rows) = self.select("SELECT dkey, id, title FROM docs", vec![])?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let dkey = g_i64(&r[0])?;
                let id = g_str(&r[1])?;
                let title = g_str(&r[2]).unwrap_or_default();
                Some((id, dkey, title))
            })
            .collect())
    }

    /// React to a `--write` that removed documents: for each id present in
    /// `baseline` but no longer in `docs`, purge any child rows a raw `DELETE
    /// FROM docs` left behind, reserve the id against reuse, and strike every
    /// inbound reference to it into a tombstone. Runs before the sync pass, the
    /// verify gate, and flush.
    pub fn cascade_removals(&mut self, pcfg: &ProjectConfig, baseline: &Baseline) -> Result<()> {
        let present = self.doc_ids()?;
        let removed: Vec<&(String, i64, String)> = baseline
            .iter()
            .filter(|(id, _, _)| !present.contains(id))
            .collect();
        if removed.is_empty() {
            return Ok(());
        }

        // Entries in the removed docs' own relation maps vanish with them; an
        // id anchored nowhere else (e.g. a tombstone of an earlier close) must
        // keep its reservation. Collect before the purge below destroys them.
        let mut carried: Vec<(String, String)> = Vec::new();
        for (_, dkey, _) in &removed {
            let (_, rows) = self.select(
                "SELECT ref_id, title FROM relations WHERE dkey = $1",
                vec![dkey.into_param()],
            )?;
            carried.extend(
                rows.iter()
                    .filter_map(|r| Some((g_str(&r[0])?, g_str(&r[1]).unwrap_or_default()))),
            );
        }

        // 1. Purge orphaned child rows: a raw `DELETE FROM docs WHERE …` only
        //    removes the `docs` row, leaving this doc's tags/relations/fm_fields
        //    behind. Clear them first so a removed doc's own outbound relations
        //    can't masquerade as a live referrer in the inbound query below.
        for (_, dkey, _) in &removed {
            self.delete_doc(*dkey)?;
        }

        // 2. Reserve the id and strike inbound references to a tombstone.
        for (id, _, title) in &removed {
            self.reserve_id(id, title)?;
            self.strike_inbound(pcfg, id, &refs::strike(title))?;
        }
        self.reserve_unanchored(&carried)?;
        Ok(())
    }

    /// Reserve `id` (with its last-known `title`) in the used-id ledger so its
    /// number is never reused, unless it is already reserved.
    pub(crate) fn reserve_id(&mut self, id: &str, title: &str) -> Result<()> {
        let already = self
            .scalar(
                "SELECT 1 FROM retired WHERE id = $1 LIMIT 1",
                vec![id.into_param()],
            )?
            .is_some();
        if already {
            return Ok(());
        }
        self.retire_id(id, title)
    }

    /// Every distinct `(ref_id, unstruck title)` currently in a relation map.
    /// Snapshotted before user `--write` DML so entries the edit destroys (e.g.
    /// a `DELETE FROM relations` hitting a closed doc's tombstone) can keep
    /// their id reservations via [`Store::reserve_unanchored`].
    pub(crate) fn relation_entries(&mut self) -> Result<Vec<(String, String)>> {
        let (_, rows) = self.select("SELECT DISTINCT ref_id, title FROM relations", vec![])?;
        Ok(rows
            .iter()
            .filter_map(|r| Some((g_str(&r[0])?, g_str(&r[1]).unwrap_or_default())))
            .collect())
    }

    /// Reserve every `(id, title)` candidate whose id is no longer anchored
    /// anywhere: not a live doc, not present in any relation map, not already in
    /// the ledger. Called when relation-map entries are destroyed (a deleted doc
    /// carried them, or `cleanup` stripped them) so a number whose only record
    /// was such an entry is still never reissued.
    pub(crate) fn reserve_unanchored(&mut self, candidates: &[(String, String)]) -> Result<()> {
        for (id, title) in candidates {
            let live = self
                .scalar(
                    "SELECT 1 FROM docs WHERE id = $1 LIMIT 1",
                    vec![id.as_str().into_param()],
                )?
                .is_some();
            if live {
                continue;
            }
            let anchored = self
                .scalar(
                    "SELECT 1 FROM relations WHERE ref_id = $1 LIMIT 1",
                    vec![id.as_str().into_param()],
                )?
                .is_some();
            if anchored {
                continue;
            }
            self.reserve_id(id, title)?;
        }
        Ok(())
    }

    /// Strike every live document's reference to `id` (in any relation map) into
    /// the `struck` tombstone value. Only existing entries are struck — a doc
    /// that never referenced `id` has nothing dangling and is left untouched.
    /// Returns the row keys of the documents that changed.
    pub(crate) fn strike_inbound(
        &mut self,
        pcfg: &ProjectConfig,
        id: &str,
        struck: &str,
    ) -> Result<Vec<i64>> {
        let (_, rows) = self.select(
            "SELECT DISTINCT dkey FROM relations WHERE ref_id = $1",
            vec![id.into_param()],
        )?;
        let dkeys: Vec<i64> = rows.iter().filter_map(|r| g_i64(&r[0])).collect();
        let mut struck_dkeys = Vec::new();
        for k in dkeys {
            let mut doc = self.doc(k)?;
            let mut changed = false;
            for field in refs::RELATION_FIELDS {
                let mut entries = refs::parse_in(&doc.frontmatter, field);
                if let Some(e) = entries.iter_mut().find(|(i, _)| i == id) {
                    if e.1 != struck {
                        e.1 = struck.to_string();
                        refs::set_in(&mut doc.frontmatter, field, &entries);
                        changed = true;
                    }
                }
            }
            if changed {
                self.put_doc(pcfg, Some(k), &doc)?;
                struck_dkeys.push(k);
            }
        }
        Ok(struck_dkeys)
    }
}
