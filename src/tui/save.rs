//! The unified write path for the edit/new screens: validate the whole edited
//! document once and persist it. Mirrors what the command cores do — stamp the
//! timestamps, run the rules engine, then write/relocate via `save_doc` — so the
//! same on-disk invariants hold. The caller runs `sync_quiet` afterward.

use std::collections::HashSet;

use crate::body;
use crate::commands::touch;
use crate::doc::Doc;
use crate::error::{usage, Result};
use crate::project::{self, Project};
use crate::rules;

/// Validate and persist `doc` (an edited existing doc or a freshly scaffolded
/// new one). Returns a usage error carrying the engine's problems when the
/// document does not satisfy the rules for its type/status — the caller surfaces
/// it on the status line instead of writing.
pub fn save_edited_doc(prj: &Project, doc: &mut Doc) -> Result<()> {
    let id = doc
        .id()
        .ok_or_else(|| usage("document has no id"))?
        .to_string();
    let tname = prj
        .pcfg
        .type_name_for_id(&id)
        .ok_or_else(|| usage(format!("unrecognized id prefix in {id}")))?
        .to_string();
    let status = doc.status().unwrap_or("").to_string();

    // The `# heading` in the body is the title's source of truth.
    doc.title = body::title(&doc.body);
    touch(&mut doc.frontmatter);

    let (docs, _) = prj.load_docs();
    let doc_ids: HashSet<String> = docs
        .iter()
        .filter_map(|d| d.id())
        .filter(|i| *i != id)
        .map(str::to_string)
        .collect();

    let problems = rules::evaluate(
        &prj.pcfg,
        &tname,
        &status,
        &doc.frontmatter,
        &doc.body,
        &doc_ids,
    );
    if !problems.is_empty() {
        return Err(usage(problems.join("; ")));
    }
    project::save_doc(prj, doc)
}
