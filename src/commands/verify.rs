use std::collections::{BTreeMap, HashMap, HashSet};

use regex::Regex;
use serde_norway::Value;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use walkdir::WalkDir;

use crate::body;
use crate::config::{FieldSpec, FieldType};
use crate::doc::Doc;
use crate::error::Result;
use crate::frontmatter::Frontmatter;
use crate::project::{id_format_re, Project, KEBAB_RE};
use crate::project_config::{CheckScope, SectionCheck, SectionKind};
use crate::refs;
use crate::rules;
use crate::Ctx;

pub fn run(ctx: &Ctx) -> Result<i32> {
    let prj = ctx.open()?;
    let (docs, mut errors) = prj.load_docs();
    let pcfg = &prj.pcfg;

    // Validate opys.toml itself, and the field specs of each type.
    for p in pcfg.validate() {
        errors.push(format!("opys.toml: {p}"));
    }
    for (tname, t) in &pcfg.types {
        check_field_specs(&t.fields, &format!("type '{tname}'"), &mut errors);
    }

    let retired = prj.retired_ids();
    let reserved = reserved_fields();
    // Corpus for `must_match` checks without a `file`, scanned at most once per
    // distinct set of `roots`.
    let mut corpus_cache: HashMap<Vec<String>, String> = HashMap::new();

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

        check_timestamps(m, id, &mut errors);
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
            if sec.kind == SectionKind::Manual {
                check_manual(d, id, &sec.heading, &mut errors);
            }
            // Universal content checks, run regardless of kind.
            for chk in &sec.checks {
                run_check(
                    d,
                    id,
                    &sec.heading,
                    chk,
                    &prj,
                    &mut corpus_cache,
                    &mut errors,
                );
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
    let mut v = vec!["id", "status", "tags", "created", "updated"];
    v.extend(refs::RELATION_FIELDS);
    v
}

/// The auto-maintained timestamp fields, when present, must be valid RFC3339
/// datetimes. They are optional: older docs predating the fields are not flagged
/// for absence (a `sync` pass backfills them).
fn check_timestamps(m: &Frontmatter, id: &str, errors: &mut Vec<String>) {
    for key in ["created", "updated"] {
        if let Some(v) = m.get(key) {
            let ok = v
                .as_str()
                .is_some_and(|s| OffsetDateTime::parse(s, &Rfc3339).is_ok());
            if !ok {
                errors.push(format!(
                    "{id}: '{key}' must be an RFC3339 datetime (e.g. 2026-06-16T14:30:00Z)"
                ));
            }
        }
    }
}

/// Every entry in each relation map (`references`, `blocked_by`, `blocks`) must
/// resolve to an existing doc, unless it is a struck-through tombstone (a closed
/// document). A blocker map may not list the doc itself.
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
                    "{id}: {what} '{tid}' does not resolve to a known document"
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

/// Run one universal [`SectionCheck`] over a document's section. `pattern`
/// parses each line into named groups; `file` (a captured path that must exist
/// under `roots`) and/or `must_match` (a regex of `${group}` substitutions that
/// must match in that file, or in the corpus when `file` is unset) then assert
/// the parsed reference points at something real.
fn run_check(
    d: &Doc,
    id: &str,
    heading: &str,
    chk: &SectionCheck,
    prj: &Project,
    corpus_cache: &mut HashMap<Vec<String>, String>,
    errors: &mut Vec<String>,
) {
    // An invalid pattern is already reported by config validation; skip here.
    let Ok(pat) = Regex::new(&chk.pattern) else {
        return;
    };

    match chk.scope {
        CheckScope::All => {
            for line in body::section(&d.body, heading).lines() {
                for caps in pat.captures_iter(line) {
                    validate_match(id, heading, chk, &caps, prj, corpus_cache, errors);
                }
            }
        }
        CheckScope::Checked => {
            for item in body::checklist_items(&d.body, heading) {
                if !item.checked {
                    continue;
                }
                let mut matched = false;
                for caps in pat.captures_iter(&item.line) {
                    matched = true;
                    validate_match(id, heading, chk, &caps, prj, corpus_cache, errors);
                }
                if !matched {
                    errors.push(format!(
                        "{id}: section '{heading}': checked item has no reference: {}",
                        item.line.trim()
                    ));
                }
            }
        }
    }
}

/// Validate one `pattern` match against a check's `file` / `must_match` rules,
/// pushing a problem (the check's `message`, or a default) when it fails.
fn validate_match(
    id: &str,
    heading: &str,
    chk: &SectionCheck,
    caps: &regex::Captures,
    prj: &Project,
    corpus_cache: &mut HashMap<Vec<String>, String>,
    errors: &mut Vec<String>,
) {
    // The text the `must_match` regex searches: a specific file, or the corpus.
    // A missing `file` is a failure in its own right (always the default
    // message — the custom `message` describes the `must_match` assertion).
    let haystack: Option<String> = match &chk.file {
        Some(group) => {
            let rel = caps.name(group).map_or("", |m| m.as_str());
            match resolve_file(prj, rel, &chk.roots) {
                Some(text) => Some(text),
                None => {
                    errors.push(format!("{id}: section '{heading}': file '{rel}' not found"));
                    return;
                }
            }
        }
        None => None,
    };

    let Some(mm) = &chk.must_match else {
        return; // `file` existence alone was the whole check.
    };
    let Ok(re) = Regex::new(&interp(mm, caps, true)) else {
        return; // invalid must_match already reported by config validation
    };
    let found = match &haystack {
        Some(text) => re.is_match(text),
        None => re.is_match(corpus(prj, &chk.roots, corpus_cache)),
    };
    if !found {
        errors.push(match &chk.message {
            Some(msg) => format!("{id}: {}", interp(msg, caps, false)),
            None => format!(
                "{id}: section '{heading}': no match for the check in {:?}",
                chk.roots
            ),
        });
    }
}

/// Substitute `${group}` references in `template` with the named captures from
/// `caps`. When `escape`, each value is `regex::escape`d (for a `must_match`
/// regex); otherwise it is inserted raw (for a human-readable `message`).
fn interp(template: &str, caps: &regex::Captures, escape: bool) -> String {
    static GROUP_RE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"\$\{(\w+)\}").unwrap());
    GROUP_RE
        .replace_all(template, |c: &regex::Captures| {
            let val = caps.name(&c[1]).map_or("", |m| m.as_str());
            if escape {
                regex::escape(val)
            } else {
                val.to_string()
            }
        })
        .into_owned()
}

/// The concatenated text of every readable file under `roots` (project-root
/// relative), memoized per `roots` set across the whole verify run.
fn corpus<'a>(
    prj: &Project,
    roots: &[String],
    cache: &'a mut HashMap<Vec<String>, String>,
) -> &'a str {
    cache.entry(roots.to_vec()).or_insert_with(|| {
        let mut text = String::new();
        for d in roots {
            for entry in WalkDir::new(prj.root.join(d))
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.file_type().is_file() {
                    if let Ok(bytes) = std::fs::read(entry.path()) {
                        text.push_str(&String::from_utf8_lossy(&bytes));
                    }
                }
            }
        }
        text
    })
}

/// Resolve a relative file path against each of `roots` (project-root relative);
/// return the first existing file's text.
fn resolve_file(prj: &Project, rel: &str, roots: &[String]) -> Option<String> {
    roots
        .iter()
        .map(|root| prj.root.join(root).join(rel))
        .find(|p| p.is_file())
        .and_then(|p| std::fs::read_to_string(p).ok())
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
