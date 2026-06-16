//! `opys import` — bulk-create documents of the `feature` type from a JSONL file
//! in a single pass.
//!
//! Each line of the file is a JSON object describing one feature. Writes still
//! go through the CLI (so invariants hold at write time), but unlike `new` the
//! whole batch shares one ID allocation and one view regeneration, which is
//! what makes migrating thousands of features tractable. The batch is
//! transactional: if any record is rejected, nothing is written.

use std::collections::HashSet;

use serde_norway::Value;

use crate::commands::maybe_sync;
use crate::doc::Doc;
use crate::error::{usage, OpysError, Result};
use crate::frontmatter::Frontmatter;
use crate::project::Project;
use crate::project_config::{DocType, ProjectConfig};
use crate::{rules, Ctx};

pub fn run(ctx: &Ctx, file: &str) -> Result<()> {
    let prj = Project::open(&ctx.root)?;
    let (docs, _) = prj.load_docs();
    let pcfg = &prj.pcfg;
    let ft = pcfg
        .types
        .get("feature")
        .ok_or_else(|| usage("import requires a 'feature' type in opys.toml"))?;
    let doc_ids: HashSet<String> = docs
        .iter()
        .filter_map(|d| d.id())
        .map(str::to_string)
        .collect();

    let text = std::fs::read_to_string(file)
        .map_err(|e| usage(format!("cannot read import file {file:?}: {e}")))?;

    // Allocate IDs sequentially from the current global max, building every
    // document in memory and collecting *all* rejections before touching disk.
    let mut next = prj.max_doc_id(&docs);
    let mut built: Vec<Doc> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        next += 1;
        let id = format!("{}-{:0pad$}", ft.prefix, next, pad = pcfg.pad);
        match build_record(&prj, ft, pcfg, &doc_ids, &id, line) {
            Ok(d) => built.push(d),
            Err(e) => errors.push(format!("line {}: {e}", i + 1)),
        }
    }

    if !errors.is_empty() {
        return Err(usage(format!(
            "import aborted — {} record(s) rejected, no files written:\n  {}",
            errors.len(),
            errors.join("\n  ")
        )));
    }
    if built.is_empty() {
        return Err(usage(format!("no records found in {file:?}")));
    }

    for d in &built {
        if let Some(parent) = d.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&d.path, d.to_text()).map_err(OpysError::from)?;
    }
    let first = built.first().and_then(Doc::id).unwrap_or("");
    let last = built.last().and_then(Doc::id).unwrap_or("");
    println!("imported {} feature(s): {first}..{last}", built.len());
    maybe_sync(ctx, &prj);
    Ok(())
}

/// Turn one JSONL line into a validated, ID-assigned feature [`Doc`].
///
/// `title` and `body` are special: `title` becomes the `# Title` heading and
/// `body` is the markdown placed beneath it. Every other key goes verbatim into
/// the frontmatter, so the schema mirrors a feature file. The engine
/// (`rules::evaluate`) is applied per record; deeper checks remain `verify`'s job.
fn build_record(
    prj: &Project,
    ft: &DocType,
    pcfg: &ProjectConfig,
    doc_ids: &HashSet<String>,
    id: &str,
    line: &str,
) -> Result<Doc> {
    // JSON is a subset of YAML, so the YAML parser reads a JSONL line directly.
    let value: Value =
        serde_norway::from_str(line).map_err(|e| usage(format!("not a valid JSON object: {e}")))?;
    let Value::Mapping(map) = value else {
        return Err(usage("expected a JSON object"));
    };

    let mut fm = Frontmatter::new();
    fm.set_str("id", id);
    let mut title: Option<String> = None;
    let mut extra_body: Option<String> = None;

    for (k, v) in map {
        let Some(key) = k.as_str() else {
            return Err(usage("record has a non-string key"));
        };
        match key {
            "id" => return Err(usage("id is auto-allocated — remove it from the record")),
            "title" => title = Some(as_string(&v, "title")?),
            "body" => extra_body = Some(as_string(&v, "body")?),
            _ => {
                fm.insert(key, v);
            }
        }
    }

    let title = title.ok_or_else(|| usage("missing required \"title\""))?;
    if title.trim().is_empty() {
        return Err(usage("\"title\" must not be empty"));
    }
    if ft.tags_required && !fm.tags_is_nonempty_list() {
        return Err(usage("\"tags\" must be a non-empty array of strings"));
    }
    if fm.contains_key("status") && fm.status().is_none() {
        return Err(usage("\"status\" must be a string"));
    }
    if !fm.contains_key("status") {
        fm.set_str("status", &ft.default_status);
    }
    let status = fm.status().expect("status set above").to_string();
    if !ft.statuses.iter().any(|s| s == &status) {
        return Err(usage(format!(
            "unknown status {status:?} (allowed: {})",
            ft.statuses.join(", ")
        )));
    }

    let body = match extra_body {
        Some(extra) if !extra.trim().is_empty() => {
            format!("# {title}\n\n{}\n", extra.trim_matches('\n'))
        }
        _ => format!("# {title}\n"),
    };

    let problems = rules::evaluate(pcfg, "feature", &status, &fm, &body, doc_ids);
    if !problems.is_empty() {
        return Err(usage(problems.join("; ")));
    }

    Ok(Doc {
        path: prj.base.join(ft.resolved_dir()).join(format!("{id}.md")),
        frontmatter: fm,
        body,
        title,
    })
}

/// Require a JSON string for a named field.
fn as_string(v: &Value, field: &str) -> Result<String> {
    v.as_str()
        .map(str::to_string)
        .ok_or_else(|| usage(format!("\"{field}\" must be a string")))
}
