use crate::commands::{maybe_sync, split_csv, touch};
use crate::doc::Doc;
use crate::error::{usage, Result};
use crate::frontmatter::Frontmatter;
use crate::project::{self, Project};
use crate::project_config::{DocType, SectionKind};
use crate::store::Store;
use crate::Ctx;
use crate::{refs, rules};

/// Build, validate, and insert a new document into the store; returns the
/// created [`Doc`] (its `path` is the canonical layout path). Does not
/// flush/print/sync.
#[allow(clippy::too_many_arguments)]
pub fn core(
    prj: &Project,
    store: &mut Store,
    type_name: &str,
    title: &str,
    tags: &str,
    status: &str,
    features: &str,
    reason: Option<&str>,
    fields: &[String],
) -> Result<Doc> {
    let pcfg = &prj.pcfg;
    let t = pcfg.types.get(type_name).ok_or_else(|| {
        let mut names: Vec<&str> = pcfg.types.keys().map(String::as_str).collect();
        names.sort_unstable();
        usage(format!(
            "unknown type {type_name:?} (configured: {})",
            names.join(", ")
        ))
    })?;

    let id = store.next_id_for(&t.prefix, pcfg.pad)?;

    // Resolve status: empty → the type's default. Reject unknown / terminal.
    let status = if status.is_empty() {
        t.default_status.as_str()
    } else {
        status
    };
    if !t.statuses.iter().any(|s| s == status) {
        return Err(usage(format!(
            "unknown status {status:?} for type '{type_name}' (allowed: {})",
            t.statuses.join(", ")
        )));
    }
    if t.terminal_statuses.iter().any(|s| s == status) {
        return Err(usage(format!(
            "cannot create a '{type_name}' as {status}: it is terminal (reached only via `close`)"
        )));
    }

    let tags = split_csv(tags);
    if t.tags_required && tags.is_empty() {
        return Err(usage("at least one tag is required (--tags a,b)"));
    }

    let mut fm = Frontmatter::new();
    fm.set_str("id", &id);
    fm.set_str("status", status);
    if !tags.is_empty() {
        fm.set_tags(&tags);
    }

    // References (e.g. linked features), resolved against the live inventory.
    let mut references = Vec::new();
    for rid in split_csv(features) {
        let title = store
            .title_of(&rid)?
            .ok_or_else(|| usage(format!("{rid} does not exist")))?;
        references.push((rid.clone(), title));
    }
    if !references.is_empty() {
        refs::set(&mut fm, &references);
    }

    // `--reason` sets the conventional `<status>_reason` field (wontfix/blocked/…).
    if let Some(r) = reason {
        fm.set_str(&format!("{status}_reason"), r);
    }
    for kv in fields {
        let (k, v) = project::parse_field(kv)?;
        fm.insert(&k, v);
    }
    touch(&mut fm);

    let body = scaffold_body(title, t);

    // Enforce the engine at write time, exactly as verify does.
    let doc_ids = store.doc_ids()?;
    let problems = rules::evaluate(pcfg, type_name, status, &fm, &body, &doc_ids);
    if !problems.is_empty() {
        return Err(usage(format!(
            "cannot create {id}: {}",
            problems.join("; ")
        )));
    }

    let doc = Doc {
        path: prj.doc_path(&id, status),
        frontmatter: fm,
        body,
        title: title.to_string(),
    };
    store.put_doc(pcfg, None, &doc)?;
    Ok(doc)
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    ctx: &Ctx,
    type_name: &str,
    title: &str,
    tags: &str,
    status: &str,
    features: &str,
    reason: Option<&str>,
    fields: &[String],
) -> Result<()> {
    let prj = ctx.open()?;
    let (mut store, _) = ctx.load(&prj)?;
    let doc = core(
        &prj, &mut store, type_name, title, tags, status, features, reason, fields,
    )?;
    println!("{}", doc.path.display());
    ctx.flush(&prj, store)?;
    maybe_sync(ctx, &prj);
    Ok(())
}

/// Scaffold the body: the title heading plus each declared section, seeded per
/// kind: a checklist gets a starter item; a structured section is scaffolded
/// from its mdprism `structure`; others just the heading.
pub fn scaffold_body(title: &str, t: &DocType) -> String {
    let mut body = format!("# {title}\n");
    for sec in t.sections.iter().filter(|s| s.required) {
        body.push_str(&format!("\n## {}\n", sec.heading));
        match sec.kind {
            SectionKind::Checklist => body.push_str("- [ ] First item\n"),
            // Scaffold the structured section from its mdprism schema.
            SectionKind::Structured => {
                if let Some(schema) = sec
                    .structure
                    .as_deref()
                    .and_then(|s| crate::mdprism::parse_schema(s).ok())
                {
                    let scaffolded = schema.scaffold();
                    body.push_str(&scaffolded);
                    if !scaffolded.ends_with('\n') {
                        body.push('\n');
                    }
                }
            }
            SectionKind::Prose | SectionKind::Log => {}
        }
    }
    body
}
