use std::collections::{BTreeMap, HashMap, HashSet};

use regex::Regex;
use serde_norway::Value;
use walkdir::WalkDir;

use crate::body;
use crate::config::{FieldSpec, FieldType, TestRefCheck};
use crate::doc::Doc;
use crate::error::Result;
use crate::frontmatter::Frontmatter;
use crate::project::{id_format_re, Project, KEBAB_RE};
use crate::project_config::SectionKind;
use crate::refs;
use crate::rules;
use crate::Ctx;

pub fn run(ctx: &Ctx) -> Result<i32> {
    let prj = ctx.open()?;
    let (docs, mut errors) = prj.load_docs();
    let pcfg = &prj.pcfg;

    // Validate a real, user-authored opys.toml, and the field specs of each type.
    if prj.base.join("opys.toml").exists() {
        for p in pcfg.validate() {
            errors.push(format!("opys.toml: {p}"));
        }
    }
    for (tname, t) in &pcfg.types {
        check_field_specs(&t.fields, &format!("type '{tname}'"), &mut errors);
    }

    let retired = prj.retired_ids();
    let index = TestIndex::build(&prj, &mut errors);
    let reserved = reserved_fields();

    // The set of ids that a `references` entry may resolve to.
    let doc_ids: HashSet<String> = docs
        .iter()
        .filter_map(|d| d.id())
        .map(str::to_string)
        .collect();

    let mut seen: HashMap<String, String> = HashMap::new();

    for d in &docs {
        let m = &d.frontmatter;
        let where_ = d.path.display().to_string();

        let Some(id) = d.id() else {
            errors.push(format!("{where_}: bad or missing id {}", pyrepr(None)));
            continue;
        };
        // The document's type is its ID prefix; an unknown prefix is an error.
        let Some(tname) = pcfg.type_name_for_id(id) else {
            let mut known: Vec<&str> = pcfg.types.values().map(|t| t.prefix.as_str()).collect();
            known.sort_unstable();
            errors.push(format!(
                "{where_}: unrecognized id prefix in {id} (expected one of: {})",
                known.join(", ")
            ));
            continue;
        };
        let t = &pcfg.types[tname];

        if !id_format_re(&t.prefix, pcfg.pad).is_match(id) {
            errors.push(format!("{where_}: bad id {id}"));
            continue;
        }
        if d.path.file_stem().and_then(|s| s.to_str()) != Some(id) {
            errors.push(format!("{where_}: filename does not match id {id}"));
        }
        if let Some(prev) = seen.get(id) {
            errors.push(format!("{where_}: duplicate id {id} (also in {prev})"));
        }
        seen.insert(id.to_string(), where_.clone());
        if retired.contains(id) {
            errors.push(format!(
                "{where_}: id {id} is retired and may not be reused"
            ));
        }

        let status = d.status();
        if !status
            .map(|s| t.statuses.iter().any(|x| x == s))
            .unwrap_or(false)
        {
            errors.push(format!("{id}: invalid status {}", pyrepr(status)));
        }

        if t.tags_required || m.contains_key("tags") {
            check_tags(m, id, &mut errors);
        }
        if d.title.is_empty() {
            errors.push(format!("{id}: missing '# Title' heading"));
        }
        if let Some(spec) = m.spec() {
            if !prj.root.join(spec).exists() {
                errors.push(format!("{id}: spec path '{spec}' does not resolve"));
            }
        }

        check_references(m, id, &doc_ids, &mut errors);
        check_custom_fields(
            &t.fields,
            &reserved,
            &format!("type '{tname}'"),
            m,
            id,
            &mut errors,
        );

        // Section-kind validators, by the type's section headings.
        for sec in &t.sections {
            match sec.kind {
                SectionKind::TestPlan => {
                    check_test_plan(d, id, &sec.heading, &index, &prj, &mut errors)
                }
                SectionKind::Manual => check_manual(d, id, &sec.heading, &mut errors),
                _ => {}
            }
        }
    }

    // The ID sequence is global: no two live docs may share a numeric part.
    check_unique_numbers(&docs, &mut errors);

    // The status-conditional guards (wontfix⇒reason, implemented⇒checked test
    // plan, blocked⇒reason/link, required links, required sections) are enforced
    // by the engine against `prj.pcfg`.
    check_rules(&prj, &docs, &doc_ids, &mut errors);

    if errors.is_empty() {
        println!("verify: OK ({} documents)", docs.len());
        Ok(0)
    } else {
        eprintln!("verify: {} problem(s)", errors.len());
        for e in &errors {
            eprintln!("  {e}");
        }
        Ok(1)
    }
}

/// Frontmatter keys allowed on any document regardless of type (everything else
/// must be a declared field of the doc's type).
fn reserved_fields() -> Vec<&'static str> {
    let mut v = vec!["id", "status", "tags"];
    v.extend(refs::RELATION_FIELDS);
    v
}

/// Every entry in each relation map (`references`, `blocked_by`, `blocks`) must
/// resolve to an existing doc, unless it is a struck-through tombstone (a closed
/// work item). A blocker map may not list the doc itself.
fn check_references(
    m: &Frontmatter,
    id: &str,
    doc_ids: &HashSet<String>,
    errors: &mut Vec<String>,
) {
    for field in refs::RELATION_FIELDS {
        let is_blocker = field == refs::BLOCKED_BY || field == refs::BLOCKS;
        for (tid, val) in refs::parse_in(m, field) {
            if is_blocker && tid == id {
                errors.push(format!("{id}: '{field}' must not list itself"));
                continue;
            }
            if !doc_ids.contains(&tid) && !refs::is_struck(&val) {
                let what = if field == refs::FIELD {
                    "reference"
                } else {
                    field
                };
                errors.push(format!(
                    "{id}: {what} '{tid}' does not resolve to a feature or work item"
                ));
            }
        }
    }
}

/// Run the universal engine against every doc using `prj.pcfg`: the conditional
/// `[[rules]]`, field-level regex patterns, and required-section presence —
/// mapping each doc to a type by its ID prefix. Findings are prefixed by the doc
/// id.
fn check_rules(prj: &Project, docs: &[Doc], doc_ids: &HashSet<String>, errors: &mut Vec<String>) {
    let pcfg = &prj.pcfg;
    let mut run = |id: Option<&str>, status: Option<&str>, m: &Frontmatter, body: &str| {
        let Some(id) = id else { return };
        let Some(type_name) = pcfg.type_name_for_id(id) else {
            return;
        };
        for p in rules::evaluate(pcfg, type_name, status.unwrap_or(""), m, body, doc_ids) {
            errors.push(format!("{id}: {p}"));
        }
        if let Some(dt) = pcfg.types.get(type_name) {
            // Field-level regex patterns (validate the value when present).
            for (fname, spec) in &dt.fields {
                if let (Some(pat), Some(val)) = (&spec.pattern, m.get_str(fname)) {
                    if Regex::new(pat).map(|re| !re.is_match(val)).unwrap_or(false) {
                        errors.push(format!("{id}: field '{fname}' must match /{pat}/"));
                    }
                }
            }
            // Required-section presence.
            for sec in &dt.sections {
                if sec.required && !body::has_section(body, &sec.heading) {
                    errors.push(format!(
                        "{id}: missing required '## {}' section",
                        sec.heading
                    ));
                }
            }
        }
    };
    for d in docs {
        run(d.id(), d.status(), &d.frontmatter, &d.body);
    }
}

/// Enforce the global ID invariant: no numeric id part is shared by two distinct
/// live docs. Exact duplicate id strings are already reported by the `seen`
/// check; this catches a number reused across prefixes (e.g. `FEAT-0003` and
/// `TASK-0003`).
fn check_unique_numbers(docs: &[Doc], errors: &mut Vec<String>) {
    let mut by_num: HashMap<u64, String> = HashMap::new();
    for id in docs.iter().filter_map(|d| d.id()) {
        let Some((_, n)) = id.rsplit_once('-') else {
            continue;
        };
        let Ok(n) = n.parse::<u64>() else { continue };
        match by_num.get(&n) {
            Some(prev) if prev != id => errors.push(format!(
                "{id}: numeric id {n} is also used by {prev} (ids must be globally unique)"
            )),
            Some(_) => {}
            None => {
                by_num.insert(n, id.to_string());
            }
        }
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

fn check_custom_fields(
    fields: &BTreeMap<String, FieldSpec>,
    reserved: &[&str],
    config_hint: &str,
    m: &Frontmatter,
    id: &str,
    errors: &mut Vec<String>,
) {
    for (k, spec) in fields {
        if spec.required && !m.contains_key(k) {
            errors.push(format!("{id}: required field '{k}' missing"));
        }
        if let Some(v) = m.get(k) {
            if spec.field_type == FieldType::Enum {
                let allowed = spec.values.join(", ");
                match v.as_str() {
                    Some(s) if spec.values.iter().any(|a| a == s) => {}
                    Some(s) => errors.push(format!(
                        "{id}: field '{k}' value '{s}' is not one of: {allowed}"
                    )),
                    None => errors.push(format!("{id}: field '{k}' should be one of: {allowed}")),
                }
            } else if !type_ok(v, spec.field_type) {
                errors.push(format!(
                    "{id}: field '{k}' should be {}",
                    spec.field_type.as_str()
                ));
            }
        }
    }
    for k in m.keys() {
        if !reserved.contains(&k) && !fields.contains_key(k) {
            errors.push(format!(
                "{id}: unknown frontmatter field '{k}' (declare it in {config_hint} [fields.{k}])"
            ));
        }
    }
}

/// Structural test-plan check: every *checked* item carries ≥1 resolvable test
/// reference. (The "implemented ⇒ a checked item" guard is now an engine rule.)
fn check_test_plan(
    d: &Doc,
    id: &str,
    heading: &str,
    index: &TestIndex,
    prj: &Project,
    errors: &mut Vec<String>,
) {
    for item in body::checklist_items(&d.body, heading) {
        if !item.checked {
            continue;
        }
        let refs = body::test_refs(&item.line);
        if refs.is_empty() {
            errors.push(format!(
                "{id}: checked test-plan item has no `test reference`: {}",
                item.line.trim()
            ));
            continue;
        }
        for r in &refs {
            if let Some(problem) = index.resolve(r, prj) {
                errors.push(format!("{id}: {problem}"));
            }
        }
    }
}

fn check_manual(d: &Doc, id: &str, heading: &str, errors: &mut Vec<String>) {
    for it in body::manual_items_in(&d.body, heading) {
        let desc: String = it.desc.chars().take(50).collect();
        if it.setup.is_none() {
            errors.push(format!("{id}: manual item missing Setup: {desc}"));
        }
        if it.steps.is_empty() {
            errors.push(format!("{id}: manual item missing numbered Steps: {desc}"));
        }
        if it.expect.is_none() {
            errors.push(format!("{id}: manual item missing Expect: {desc}"));
        }
        let as_item = format!("- {}", it.desc);
        if as_item.to_lowercase().starts_with("- [x] ")
            || it.desc.starts_with("[ ]")
            || it.desc.starts_with("[x]")
        {
            errors.push(format!("{id}: manual items must not be checkboxes: {desc}"));
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
        match prj.pcfg.tests.reference_check {
            TestRefCheck::None => TestIndex::Off,
            TestRefCheck::Grep => {
                let mut corpus = String::new();
                for (_, text) in scan_files(prj) {
                    corpus.push_str(&text);
                }
                TestIndex::Grep(corpus)
            }
            TestRefCheck::Extract => {
                let Some(pat) = &prj.pcfg.tests.name_pattern else {
                    errors.push(
                        "config: tests.reference_check = \"extract\" requires tests.name_pattern"
                            .into(),
                    );
                    return TestIndex::Off;
                };
                let re = match Regex::new(pat) {
                    Ok(re) => re,
                    Err(e) => {
                        errors.push(format!("config: invalid tests.name_pattern: {e}"));
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
                TestIndex::Extract { names, re }
            }
        }
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
                    prj.pcfg.tests.search_paths
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
                            prj.pcfg.tests.search_paths
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
    for d in &prj.pcfg.tests.search_paths {
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
    for d in &prj.pcfg.tests.search_paths {
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
        // Enum values are strings; membership is checked separately, with access
        // to the declared `values` (see `check_custom_fields`).
        FieldType::Enum => v.is_string(),
    }
}

/// Validate the field declarations themselves (run once per config): an `enum`
/// field must declare a non-empty `values` set.
fn check_field_specs(
    fields: &BTreeMap<String, FieldSpec>,
    config_hint: &str,
    errors: &mut Vec<String>,
) {
    for (k, spec) in fields {
        if spec.field_type == FieldType::Enum && spec.values.is_empty() {
            errors.push(format!(
                "config: field '{k}' is enum but declares no values ({config_hint} [fields.{k}].values)"
            ));
        }
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
