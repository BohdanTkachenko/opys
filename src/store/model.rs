//! Id-reservation and tombstone helpers on the store.
//!
//! The lifecycle commands (`close`, `retire`, `cleanup`) end a document's life
//! in more places than its own file: the number is reserved in the retired
//! ledger so it is never reissued, and every inbound reference to it is struck
//! into a `~~title~~` tombstone so no live link dangles. Those two effects live
//! here rather than in each command, because a reference map entry can be the
//! last anchor holding an id's reservation.

use crate::error::Result;
use crate::project_config::ProjectConfig;
use crate::refs;

use super::{g_i64, IntoParam, Store};

impl Store {
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

    /// Reserve every `(id, title)` candidate whose id is no longer anchored
    /// anywhere: not a live doc, not present in any relation map, not already in
    /// the ledger. Called when relation-map entries are destroyed (a closed or
    /// retired doc carried them, or `cleanup` stripped them) so a number whose
    /// only record was such an entry is still never reissued.
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
