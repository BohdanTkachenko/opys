//! `opys import` — bulk-create features from a JSONL file in a single pass.
//!
//! Each line of the file is a JSON object describing one feature. Writes still
//! go through the CLI (so invariants hold at write time), but unlike `new` the
//! whole batch shares one ID allocation and one view regeneration, which is
//! what makes migrating thousands of features tractable. The batch is
//! transactional: if any record is rejected, nothing is written.

use serde_norway::Value;

use crate::body;
use crate::commands::maybe_sync;
use crate::error::{usage, OpysError, Result};
use crate::feature::Feature;
use crate::frontmatter::Frontmatter;
use crate::project::Project;
use crate::Ctx;

pub fn run(ctx: &Ctx, file: &str) -> Result<()> {
    let prj = Project::open(&ctx.root, &ctx.dir)?;
    let (feats, _) = prj.load();
    let statuses = prj.cfg.statuses();

    let text = std::fs::read_to_string(file)
        .map_err(|e| usage(format!("cannot read import file {file:?}: {e}")))?;

    // Allocate IDs sequentially from the current max, building every feature in
    // memory and collecting *all* rejections before touching disk.
    let mut next = prj.max_id_number(&feats);
    let mut built: Vec<Feature> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        next += 1;
        match build_record(&prj, &statuses, next, line) {
            Ok(f) => built.push(f),
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

    for f in &built {
        std::fs::write(&f.path, f.to_text()).map_err(OpysError::from)?;
    }
    let first = built.first().and_then(Feature::id).unwrap_or("");
    let last = built.last().and_then(Feature::id).unwrap_or("");
    println!("imported {} feature(s): {first}..{last}", built.len());
    maybe_sync(ctx, &prj);
    Ok(())
}

/// Turn one JSONL line into a validated, ID-assigned [`Feature`].
///
/// `title` and `body` are special: `title` becomes the `# Title` heading and
/// `body` is the markdown placed beneath it. Every other key goes verbatim
/// into the frontmatter (`status`, `tags`, `spec`, and any custom fields), so
/// the schema mirrors a feature file. The write-time status guards from `new`
/// / `set-status` are re-applied here; deeper checks remain `verify`'s job.
fn build_record(prj: &Project, statuses: &[String], id_num: u64, line: &str) -> Result<Feature> {
    // JSON is a subset of YAML, so the YAML parser reads a JSONL line directly
    // (and JSON's mandatory quoting sidesteps the unquoted-colon footgun).
    let value: Value =
        serde_norway::from_str(line).map_err(|e| usage(format!("not a valid JSON object: {e}")))?;
    let Value::Mapping(map) = value else {
        return Err(usage("expected a JSON object"));
    };

    let id = prj.format_id(id_num);
    let mut fm = Frontmatter::new();
    fm.set_str("id", &id);
    let mut title: Option<String> = None;
    let mut extra_body: Option<String> = None;

    for (k, v) in map {
        let Some(key) = k.as_str() else {
            return Err(usage("record has a non-string key"));
        };
        match key {
            "id" => return Err(usage("id is auto-allocated — remove it from the record")),
            "title" => {
                title = Some(as_string(&v, "title")?);
            }
            "body" => {
                extra_body = Some(as_string(&v, "body")?);
            }
            _ => {
                fm.insert(key, v);
            }
        }
    }

    let title = title.ok_or_else(|| usage("missing required \"title\""))?;
    if title.trim().is_empty() {
        return Err(usage("\"title\" must not be empty"));
    }
    if !fm.tags_is_nonempty_list() {
        return Err(usage("\"tags\" must be a non-empty array of strings"));
    }
    if fm.contains_key("status") && fm.status().is_none() {
        return Err(usage("\"status\" must be a string"));
    }
    if !fm.contains_key("status") {
        fm.set_str("status", "planned");
    }
    let status = fm.status().expect("status set above").to_string();

    let body = match extra_body {
        Some(extra) if !extra.trim().is_empty() => {
            format!("# {title}\n\n{}\n", extra.trim_matches('\n'))
        }
        _ => format!("# {title}\n"),
    };

    if !statuses.iter().any(|s| s == &status) {
        return Err(usage(format!(
            "unknown status {status:?} (allowed: {})",
            statuses.join(", ")
        )));
    }
    if status == "wontfix" && fm.wontfix_reason().is_none() {
        return Err(usage("wontfix requires a \"wontfix_reason\""));
    }
    if status == "implemented" && !body::test_plan_items(&body).iter().any(|i| i.checked) {
        return Err(usage(
            "implemented requires a checked test-plan item in \"body\"",
        ));
    }

    Ok(Feature {
        path: prj.path_for(&id),
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
