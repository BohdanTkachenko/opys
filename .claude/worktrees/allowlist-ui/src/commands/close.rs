//! `opys close` — finish a document of a type that has a terminal status,
//! deleting its file and striking every reference to it into a tombstone.

use std::collections::HashSet;

use crate::body;
use crate::commands::{expand_ids, for_each_id, maybe_sync};
use crate::error::{usage, Result};
use crate::frontmatter::Frontmatter;
use crate::project::Project;
use crate::project_config::SectionKind;
use crate::store::{g_i64, IntoParam, Store};
use crate::{refs, Ctx};

/// Close `id`: delete its file and strike every reference to it. Does not flush/
/// print/sync — the shared core for the CLI wrapper and the TUI. Striking other
/// docs' relation maps is housekeeping, so it does not bump their `updated`.
pub fn core(prj: &Project, store: &mut Store, id: &str, force: bool) -> Result<()> {
    let tname = prj
        .pcfg
        .type_name_for_id(id)
        .ok_or_else(|| usage(format!("unrecognized id prefix in {id}")))?
        .to_string();
    let t = &prj.pcfg.types[&tname];
    if t.terminal_statuses.is_empty() {
        return Err(usage(format!(
            "type '{tname}' has no terminal status, so '{id}' cannot be closed"
        )));
    }

    let dkey = store.dkey_of(id)?;
    let closing = store.doc(dkey)?;

    // Don't close with unfinished work unless forced: any required checklist
    // section must be fully checked.
    if !force {
        for sec in t
            .sections
            .iter()
            .filter(|s| s.required && s.kind == SectionKind::Checklist)
        {
            if body::checklist_items(&closing.body, &sec.heading)
                .iter()
                .any(|i| !i.checked)
            {
                return Err(usage(format!(
                    "cannot close — unchecked items remain in '## {}' (check them, or pass --force)",
                    sec.heading
                )));
            }
        }
    }

    let struck = refs::strike(&closing.title);
    // Targets this doc references should carry a struck `references` tombstone
    // even if their reverse link is not yet present.
    let ref_targets: HashSet<String> = refs::parse(&closing.frontmatter)
        .into_iter()
        .map(|(tid, _)| tid)
        .collect();

    // The docs to update: everything that references the closing doc (any
    // field), plus every live target it references (for the reverse tombstone).
    let (_, rows) = store.select(
        "SELECT DISTINCT dkey FROM relations WHERE ref_id = $1 AND dkey <> $2",
        vec![id.into_param(), dkey.into_param()],
    )?;
    let mut affected: Vec<i64> = rows.iter().filter_map(|r| g_i64(&r[0])).collect();
    for tid in &ref_targets {
        if let Some(k) = store.dkey_opt(tid)? {
            if k != dkey && !affected.contains(&k) {
                affected.push(k);
            }
        }
    }

    for k in affected {
        let mut doc = store.doc(k)?;
        let add_ref = ref_targets.contains(doc.id().unwrap_or(""));
        if strike_everywhere(&mut doc.frontmatter, id, &struck, add_ref) {
            store.put_doc(&prj.pcfg, Some(k), &doc)?;
        }
    }

    store.delete_doc(dkey)?;
    // The tombstones struck above also reserve the id, but they are not
    // durable: the doc may be unreferenced (no tombstone at all), and `cleanup`
    // strips tombstones later. The ledger entry is the reservation of record.
    store.reserve_id(id, &closing.title)?;
    // Relation entries living only in the closed doc's own maps (e.g. the
    // tombstone of an earlier close) are destroyed with its file — reserve any
    // id they were the last anchor for.
    let carried: Vec<(String, String)> = refs::RELATION_FIELDS
        .iter()
        .flat_map(|f| refs::parse_in(&closing.frontmatter, f))
        .map(|(tid, val)| {
            let title = refs::unstrike(&val).to_string();
            (tid, title)
        })
        .collect();
    store.reserve_unanchored(&carried)?;
    Ok(())
}

pub fn run(ctx: &Ctx, ids: &str, force: bool) -> Result<()> {
    let prj = ctx.open()?;
    let ids = expand_ids(ids)?;
    let (mut store, _) = ctx.load(&prj)?;
    let res = for_each_id(&ids, |id| {
        core(&prj, &mut store, id, force)?;
        println!("closed {id} (deleted; references struck through)");
        Ok(())
    });
    ctx.flush(&prj, store)?;
    maybe_sync(ctx, &prj);
    res
}

/// Strike `id` to a tombstone in every relation map that lists it; for
/// `references`, optionally add a struck entry when `add_ref` and none is
/// present. Returns whether anything changed.
fn strike_everywhere(fm: &mut Frontmatter, id: &str, struck: &str, add_ref: bool) -> bool {
    let mut changed = false;
    for field in refs::RELATION_FIELDS {
        let add = add_ref && field == refs::FIELD;
        changed |= strike_in_field(fm, field, id, struck, add);
    }
    changed
}

fn strike_in_field(
    fm: &mut Frontmatter,
    field: &str,
    id: &str,
    struck: &str,
    add_if_missing: bool,
) -> bool {
    let mut entries = refs::parse_in(fm, field);
    if let Some(e) = entries.iter_mut().find(|(i, _)| i == id) {
        if e.1 == struck {
            return false;
        }
        e.1 = struck.to_string();
    } else if add_if_missing {
        entries.push((id.to_string(), struck.to_string()));
    } else {
        return false;
    }
    refs::set_in(fm, field, &entries);
    true
}
