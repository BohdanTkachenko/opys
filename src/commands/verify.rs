use std::collections::{HashMap, HashSet};

use regex::Regex;
use serde_norway::Value;
use walkdir::WalkDir;

use crate::body;
use crate::config::FieldType;
use crate::error::Result;
use crate::feature::Feature;
use crate::frontmatter::{Frontmatter, RESERVED_FIELDS};
use crate::project::{id_format_re, Project, KEBAB_RE};
use crate::Ctx;

pub fn run(ctx: &Ctx) -> Result<i32> {
    let prj = ctx.open()?;
    let (feats, mut errors) = prj.load();

    let statuses = prj.cfg.statuses();
    let retired = prj.retired_ids();
    let id_rx = id_format_re(&prj.cfg.prefix, prj.cfg.pad);
    let index = TestIndex::build(&prj, &mut errors);

    let mut seen: HashMap<String, String> = HashMap::new();

    for f in &feats {
        let m = &f.frontmatter;
        let where_ = f.path.display().to_string();

        let fid = match f.id() {
            Some(id) if id_rx.is_match(id) => id,
            other => {
                errors.push(format!("{where_}: bad or missing id {}", pyrepr(other)));
                continue;
            }
        };

        if f.path.file_stem().and_then(|s| s.to_str()) != Some(fid) {
            errors.push(format!("{where_}: filename does not match id {fid}"));
        }
        if let Some(prev) = seen.get(fid) {
            errors.push(format!("{where_}: duplicate id {fid} (also in {prev})"));
        }
        seen.insert(fid.to_string(), where_.clone());
        if retired.contains(fid) {
            errors.push(format!(
                "{where_}: id {fid} is retired and may not be reused"
            ));
        }

        let status = f.status();
        if !status
            .map(|s| statuses.iter().any(|x| x == s))
            .unwrap_or(false)
        {
            errors.push(format!("{fid}: invalid status {}", pyrepr(status)));
        }

        check_tags(m, fid, &mut errors);

        if f.title.is_empty() {
            errors.push(format!("{fid}: missing '# Title' heading"));
        }
        if status == Some("wontfix") && m.wontfix_reason().is_none() {
            errors.push(format!("{fid}: wontfix requires wontfix_reason"));
        }
        if let Some(spec) = m.spec() {
            if !prj.root.join(spec).exists() {
                errors.push(format!("{fid}: spec path '{spec}' does not resolve"));
            }
        }

        check_custom_fields(&prj, m, fid, &mut errors);
        check_test_plan(f, fid, status, &index, &prj, &mut errors);
        check_manual(f, fid, &mut errors);
    }

    if errors.is_empty() {
        println!("verify: OK ({} features)", feats.len());
        Ok(0)
    } else {
        eprintln!("verify: {} problem(s)", errors.len());
        for e in &errors {
            eprintln!("  {e}");
        }
        Ok(1)
    }
}

fn check_tags(m: &Frontmatter, fid: &str, errors: &mut Vec<String>) {
    if !m.tags_is_nonempty_list() {
        errors.push(format!("{fid}: tags must be a non-empty list"));
        return;
    }
    if let Some(Value::Sequence(seq)) = m.get("tags") {
        for t in seq {
            let display = tag_display(t);
            if !KEBAB_RE.is_match(&display) {
                errors.push(format!(
                    "{fid}: tag '{display}' is not lowercase kebab-case"
                ));
            }
        }
    }
}

fn check_custom_fields(prj: &Project, m: &Frontmatter, fid: &str, errors: &mut Vec<String>) {
    for (k, spec) in &prj.cfg.fields {
        if spec.required && !m.contains_key(k) {
            errors.push(format!("{fid}: required field '{k}' missing"));
        }
        if let Some(v) = m.get(k) {
            if !type_ok(v, spec.field_type) {
                errors.push(format!(
                    "{fid}: field '{k}' should be {}",
                    spec.field_type.as_str()
                ));
            }
        }
    }
    for k in m.keys() {
        if !RESERVED_FIELDS.contains(&k) && !prj.cfg.fields.contains_key(k) {
            errors.push(format!(
                "{fid}: unknown frontmatter field '{k}' (declare it in features/_config.toml [fields.{k}])"
            ));
        }
    }
}

fn check_test_plan(
    f: &Feature,
    fid: &str,
    status: Option<&str>,
    index: &TestIndex,
    prj: &Project,
    errors: &mut Vec<String>,
) {
    let mut checked_any = false;
    for item in body::test_plan_items(&f.body) {
        let refs = body::test_refs(&item.line);
        if !item.checked {
            continue;
        }
        checked_any = true;
        if refs.is_empty() {
            errors.push(format!(
                "{fid}: checked test-plan item has no `test reference`: {}",
                item.line.trim()
            ));
            continue;
        }
        for r in &refs {
            if let Some(problem) = index.resolve(r, prj) {
                errors.push(format!("{fid}: {problem}"));
            }
        }
    }
    if status == Some("implemented") && !checked_any {
        errors.push(format!("{fid}: implemented but no checked test-plan item"));
    }
}

fn check_manual(f: &Feature, fid: &str, errors: &mut Vec<String>) {
    for it in body::manual_items(&f.body) {
        let d: String = it.desc.chars().take(50).collect();
        if it.setup.is_none() {
            errors.push(format!("{fid}: manual item missing Setup: {d}"));
        }
        if it.steps.is_empty() {
            errors.push(format!("{fid}: manual item missing numbered Steps: {d}"));
        }
        if it.expect.is_none() {
            errors.push(format!("{fid}: manual item missing Expect: {d}"));
        }
        let as_item = format!("- {}", it.desc);
        if as_item.to_lowercase().starts_with("- [x] ")
            || it.desc.starts_with("[ ]")
            || it.desc.starts_with("[x]")
        {
            errors.push(format!("{fid}: manual items must not be checkboxes: {d}"));
        }
    }
}

/// How `verify` checks that a referenced test exists.
enum TestIndex {
    /// No existence checking.
    Off,
    /// Test name must appear as a substring anywhere under `test_search_paths`.
    Grep(String),
    /// Test names extracted via the configured regex.
    Extract { names: HashSet<String>, re: Regex },
}

impl TestIndex {
    fn build(prj: &Project, errors: &mut Vec<String>) -> TestIndex {
        if prj.cfg.grep_mode() {
            let mut corpus = String::new();
            for (_, text) in scan_files(prj) {
                corpus.push_str(&text);
            }
            return TestIndex::Grep(corpus);
        }
        if prj.cfg.extract_mode() {
            let Some(pat) = &prj.cfg.test_name_pattern else {
                errors.push(
                    "config: test_reference_check = \"extract\" requires test_name_pattern".into(),
                );
                return TestIndex::Off;
            };
            let re = match Regex::new(pat) {
                Ok(re) => re,
                Err(e) => {
                    errors.push(format!("config: invalid test_name_pattern: {e}"));
                    return TestIndex::Off;
                }
            };
            let mut names = HashSet::new();
            for (_, text) in scan_files(prj) {
                for c in re.captures_iter(&text) {
                    if let Some(g) = c.get(1) {
                        names.insert(g.as_str().to_string());
                    }
                }
            }
            return TestIndex::Extract { names, re };
        }
        TestIndex::Off
    }

    /// Returns a problem message if the reference does not resolve.
    fn resolve(&self, reference: &str, prj: &Project) -> Option<String> {
        let (prefix, name) = match reference.rsplit_once("::") {
            Some((p, n)) => (p, n),
            None => ("", reference),
        };
        match self {
            TestIndex::Off => None,
            TestIndex::Grep(corpus) => (!corpus.contains(name)).then(|| {
                format!(
                    "test reference `{reference}` not found under {:?}",
                    prj.cfg.test_search_paths
                )
            }),
            TestIndex::Extract { names, re } => {
                if is_path_ref(prefix) {
                    match resolve_file(prj, prefix) {
                        None => Some(format!("test file `{prefix}` not found")),
                        Some(text) => {
                            let in_file = re
                                .captures_iter(&text)
                                .filter_map(|c| c.get(1))
                                .any(|g| g.as_str() == name);
                            (!in_file).then(|| format!("test `{name}` not defined in `{prefix}`"))
                        }
                    }
                } else {
                    (!names.contains(name)).then(|| {
                        format!(
                            "test reference `{reference}` not found under {:?}",
                            prj.cfg.test_search_paths
                        )
                    })
                }
            }
        }
    }
}

/// All readable files under `test_search_paths`, as (path, contents).
fn scan_files(prj: &Project) -> Vec<(std::path::PathBuf, String)> {
    let mut out = Vec::new();
    for d in &prj.cfg.test_search_paths {
        for entry in WalkDir::new(prj.root.join(d))
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                if let Ok(bytes) = std::fs::read(entry.path()) {
                    out.push((
                        entry.path().to_path_buf(),
                        String::from_utf8_lossy(&bytes).into_owned(),
                    ));
                }
            }
        }
    }
    out
}

fn is_path_ref(prefix: &str) -> bool {
    prefix.contains('/') || prefix.contains('.')
}

/// Resolve a `path::name` file prefix against the root and the search paths.
fn resolve_file(prj: &Project, prefix: &str) -> Option<String> {
    let mut candidates = vec![prj.root.join(prefix)];
    for d in &prj.cfg.test_search_paths {
        candidates.push(prj.root.join(d).join(prefix));
    }
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .and_then(|p| std::fs::read_to_string(p).ok())
}

fn type_ok(v: &Value, want: FieldType) -> bool {
    match want {
        FieldType::String => v.is_string(),
        FieldType::List => v.is_sequence(),
        FieldType::Bool => v.is_bool(),
        FieldType::Int => matches!(v, Value::Number(n) if n.is_i64() || n.is_u64()),
    }
}

/// Display form of a tag value for the kebab-case check.
fn tag_display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// Python-`repr`-like rendering for error messages.
fn pyrepr(v: Option<&str>) -> String {
    match v {
        Some(s) => format!("'{s}'"),
        None => "None".to_string(),
    }
}
