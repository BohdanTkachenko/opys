//! `opys close` — finish a document of a type that has a terminal status,
//! deleting its file and striking every reference to it into a tombstone.

use std::collections::HashSet;

use crate::body;
use crate::commands::{expand_ids, for_each_id, maybe_sync};
use crate::error::{usage, Result};
use crate::frontmatter::Frontmatter;
use crate::project::{self, Project};
use crate::project_config::SectionKind;
use crate::{refs, Ctx};

/// Close `id`: delete its file and strike every reference to it. Does not print
/// or sync — the shared core for the CLI wrapper and the TUI. Striking other
/// docs' relation maps is housekeeping, so it does not bump their `updated`.
pub fn core(prj: &Project, id: &str, force: bool) -> Result<()> {
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

    let (mut docs, _) = prj.load_docs();
    let idx = docs
        .iter()
        .position(|d| d.id() == Some(id))
        .ok_or_else(|| crate::error::OpysError::NotFound { id: id.to_string() })?;

    // Don't close with unfinished work unless forced: any required checklist
    // section must be fully checked.
    if !force {
        for sec in t
            .sections
            .iter()
            .filter(|s| s.required && s.kind == SectionKind::Checklist)
        {
            if body::checklist_items(&docs[idx].body, &sec.heading)
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

    let path = docs[idx].path.clone();
    let struck = refs::strike(&docs[idx].title);
    // Targets this doc references should carry a struck `references` tombstone
    // even if their reverse link is not yet present.
    let ref_targets: HashSet<String> = refs::parse(&docs[idx].frontmatter)
        .into_iter()
        .map(|(tid, _)| tid)
        .collect();

    for (i, d) in docs.iter_mut().enumerate() {
        if i == idx {
            continue;
        }
        let add_ref = ref_targets.contains(d.id().unwrap_or(""));
        if strike_everywhere(&mut d.frontmatter, id, &struck, add_ref) {
            project::save_doc(prj, d)?;
        }
    }

    std::fs::remove_file(&path)?;
    Ok(())
}

pub fn run(ctx: &Ctx, ids: &str, force: bool) -> Result<()> {
    let prj = ctx.open()?;
    let ids = expand_ids(ids)?;
    let res = for_each_id(&ids, |id| {
        core(&prj, id, force)?;
        println!("closed {id} (deleted; references struck through)");
        Ok(())
    });
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
