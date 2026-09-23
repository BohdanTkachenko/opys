use std::collections::{BTreeMap, HashMap, HashSet};

use regex::Regex;
use serde_norway::Value;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use walkdir::WalkDir;

use crate::body;
use crate::commands::print_lines;
use crate::config::{FieldSpec, FieldType};
use crate::doc::Doc;
use crate::error::Result;
use crate::frontmatter::Frontmatter;
use crate::project::{id_format_re, Project, KEBAB_RE};
use crate::project_config::{CheckScope, DocType, SectionCheck, SectionKind, SectionSpec};
use crate::refs;
use crate::rules;
use crate::Ctx;

/// The CI gate. Exit `0` clean, `1` content problems, `3`
/// [observations](crate::observe) and nothing else; a real failure is an
/// `OpysError` the binary maps to `2`.
///
/// With `fix`, the fixable observations are performed first — as ordinary
/// transitions through `set_status::core`, then flushed, synced, and
/// re-verified, so the code reports what *remains*. Only then is verify a
/// mutating command; the inventory flock it has always taken through
/// `Backend::load` is what makes that safe.
pub fn run(ctx: &Ctx, fix: bool) -> Result<i32> {
    let prj = ctx.open()?;
    // Read the corpus through the store; unparsable files surface as errors
    // exactly as `load_docs` reported them, and the reconstructed docs are
    // byte-identical to the parsed ones (the store round-trip is a fixpoint).
    let (mut store, parse_errors) = ctx.load(&prj)?;
    let mut docs: Vec<Doc> = store.all_docs()?.into_iter().map(|(_, d)| d).collect();
    let mut errors = collect_problems(&prj, &docs, parse_errors);
    // …plus the checks that read the medium's own text (see below). The command
    // has not written anything, so what the backend hands back is still the
    // corpus the store was built from.
    errors.extend(spelling_problems(&docs, ctx.backend.as_ref()));
    let mut observations = crate::observe::observations(&prj, &docs, crate::observe::now());

    // A fix is an ordinary `set-status` along the graph the config declares, and
    // `config validate` is what checks that graph leaves a fixed document a way
    // back (ADR-0101 decision 6). Performing fixes while that check is failing
    // would park documents in a dead end named by the very run that reported the
    // misconfiguration — so `--fix` stands down and says so, once, on stderr.
    // The problems themselves are already in `errors` (`collect_problems` runs
    // `pcfg.validate()`), so the code is 1 either way.
    if fix && !prj.pcfg.validate().is_empty() {
        eprintln!(
            "verify: --fix skipped — opys.toml has problems, and a fix follows the \
             transitions it declares; run `opys config validate`"
        );
    } else if fix {
        let fixes = crate::observe::apply_fixes(&prj, &mut store, &observations);
        if fixes.applied.is_empty() {
            // Nothing moved: no flush, and — the load-bearing half — no sync.
            // The flush alone would write nothing anyway (it writes the
            // documents a fix dirtied, and there are none), but the sync pass
            // that follows it below backfills the auto-maintained timestamps
            // and rewrites every document it touches canonically. Running the
            // pair unconditionally would turn `verify --fix` into a silent
            // `sync` over a corpus it found nothing to fix in.
            errors.extend(fixes.refused);
        } else {
            // Flush, then announce, then sync — the order every mutating
            // command uses: nothing claims a fix the flush did not land, and the
            // flush is what drops the inventory flock, which `sync` then takes
            // again for itself.
            ctx.flush(&prj, store)?;
            print_lines(&fixes.applied)?;
            crate::commands::maybe_sync(ctx, &prj);
            // Re-verify from the medium: the relocation happened at flush, and
            // `spelling_problems` reads the stored text.
            let (mut store, parse_errors) = ctx.load(&prj)?;
            docs = store.all_docs()?.into_iter().map(|(_, d)| d).collect();
            errors = collect_problems(&prj, &docs, parse_errors);
            errors.extend(spelling_problems(&docs, ctx.backend.as_ref()));
            errors.extend(fixes.refused);
            observations = crate::observe::observations(&prj, &docs, crate::observe::now());
        }
    }

    report(&docs, &errors, &observations)
}

/// Print the outcome and return the exit code: problems first, observations
/// after them, both on stderr; the clean line on stdout.
///
/// One function, so the block's exact bytes have one place to change.
///
/// `verify: OK` is printed only for exit `0`. A corpus with observations *is*
/// free of content problems, but a nonzero exit whose stdout says `OK` is the
/// one line a CI script would grep and get wrong; what it found is on stderr
/// with the problems, and the code says which of the two it was.
fn report(
    docs: &[Doc],
    errors: &[String],
    observations: &[crate::observe::Observation],
) -> Result<i32> {
    if !errors.is_empty() {
        eprintln!("verify: {} problem(s)", errors.len());
        for e in errors {
            eprintln!("  {e}");
        }
    } else if observations.is_empty() {
        print_lines([format!("verify: OK ({} documents)", docs.len())])?;
    }
    if !observations.is_empty() {
        eprintln!("verify: {} observation(s)", observations.len());
        for o in observations {
            eprintln!("  {o}");
        }
    }
    Ok(match (errors.is_empty(), observations.is_empty()) {
        (true, true) => 0,
        (true, false) => 3,
        (false, _) => 1,
    })
}

/// The problems only the *stored* text can reveal: invariants about how a value
/// is **written**, which the parsed model has already normalized away. There is
/// one today — format.md requires every `history` key to be quoted, and both
/// spellings parse to the identical string (see [`crate::history::unquoted_keys`]).
///
/// Deliberately outside [`collect_problems`]: that gate also runs over documents
/// that exist only in memory (the "no worse than before" gate in
/// [`crate::commands::edit`], which judges the *mutated* store), where the
/// medium's text is *not* the text being judged — and opys's own writer always
/// emits the quoted form, so a check against stale bytes could only produce a
/// false verdict. Callers that do read the durable corpus (`verify`, the node's
/// per-corpus problem list) add this on top.
pub fn spelling_problems(docs: &[Doc], backend: &dyn crate::backend::Backend) -> Vec<String> {
    let mut errors = Vec::new();
    for d in docs {
        let (Some(id), Some(stored)) = (d.id(), backend.stored_text(&d.path)) else {
            continue;
        };
        for key in crate::history::unquoted_keys(&d.frontmatter, &stored) {
            errors.push(format!(
                "{id}: history key '{key}' is written unquoted (a bare RFC3339 \
                 scalar is read as a timestamp, not a string, by a YAML 1.1 \
                 reader); quote it: \"{key}\": <status>"
            ));
        }
    }
    errors
}

/// Run every integrity check over `docs`, appending to the parse errors already
/// collected at load. Shared by the `verify` command and the "no worse" gate in
/// `commands::edit` (an edit is only flushed if it adds nothing to this list).
/// Never returns an `OpysError` — content problems are data, not failures.
pub fn collect_problems(prj: &Project, docs: &[Doc], parse_errors: Vec<String>) -> Vec<String> {
    let mut errors = parse_errors;
    let pcfg = &prj.pcfg;

    // Validate opys.toml itself, and the field specs of each type.
    for p in pcfg.validate() {
        errors.push(format!("opys.toml: {p}"));
    }
    for (tname, t) in &pcfg.types {
        check_field_specs(&t.fields, &format!("type '{tname}'"), &mut errors);
    }

    // A corrupt ledger is already reported as a load error (it rides in via
    // `parse_errors`); the reuse check then just runs against an empty set.
    let retired = prj.retired_ids().unwrap_or_default();
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

    for d in docs {
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
        check_history(m, id, &mut errors);
        check_transitions(m, id, tname, t, &mut errors);
        check_references(m, id, &doc_ids, &mut errors);
        for snip in crate::links::nested_links(&d.body) {
            errors.push(format!(
                "{id}: nested markdown link in body (fix by hand, keeping the innermost link): {snip}"
            ));
        }
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
            if sec.kind == SectionKind::Structured {
                check_structured(d, id, sec, &mut errors);
            }
            if sec.kind == SectionKind::Log {
                check_log(d, id, sec, &mut errors);
            }
            // Universal content checks, run regardless of kind.
            for chk in &sec.checks {
                run_check(
                    d,
                    id,
                    &sec.heading,
                    chk,
                    prj,
                    &mut corpus_cache,
                    &mut errors,
                );
            }
        }
    }

    // The ID sequence is global: no two live docs may share a numeric part.
    check_unique_numbers(docs, &mut errors);

    // The status-conditional guards (wontfix⇒reason, implemented⇒checked test
    // plan, required links, required sections) are enforced by the engine
    // against `prj.pcfg`.
    check_rules(prj, docs, &doc_ids, &mut errors);

    // Each configured `[[stats]]` section must run cleanly against the real
    // corpus (its SQL parses and executes against the corpus view).
    check_stats(prj, docs, &mut errors);

    errors
}

/// Run each configured `[[stats]]` SQL query against the live corpus, reporting
/// any failure (a query that fails to parse or execute against the corpus view)
/// as a content problem.
fn check_stats(prj: &Project, docs: &[Doc], errors: &mut Vec<String>) {
    if prj.pcfg.stats.is_empty() {
        return;
    }
    let refs: Vec<&Doc> = docs.iter().collect();
    let corpus = crate::commands::stats::corpus_json(&prj.pcfg, &refs);
    // Build the corpus DB once and run every stat against it (collect all
    // problems — verify reports them together).
    let mut db = match crate::commands::stats::build_db(&corpus) {
        Ok(db) => db,
        Err(e) => {
            errors.push(format!("stats: {e}"));
            return;
        }
    };
    for spec in &prj.pcfg.stats {
        if let Err(problem) = crate::commands::stats::render_stat_on(&mut db, spec) {
            errors.push(problem);
        }
    }
}

/// Frontmatter keys allowed on any document regardless of type (everything else
/// must be a declared field of the doc's type).
fn reserved_fields() -> Vec<&'static str> {
    let mut v = vec![
        "id",
        "status",
        "tags",
        "created",
        "updated",
        crate::history::FIELD,
    ];
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

/// The `history` lifecycle map (written by `new`/`set-status`/`apply`/`import`,
/// never by hand) must be present and well formed. Four checks, in order:
///
/// 1. it is a lifecycle map at all — `history::is_map`, the *one* shape rule the
///    store and `history::entries` use too. A value that carries entries but
///    fails it is reported through `history::botched`, which names the offending
///    key or entry — or, when the value is not a mapping at all (a hand-migrated
///    YAML list, say), its shape. Only a value carrying no entries whatever
///    (absent, `{}`, `[]`, a bare scalar) says `missing history`, the phrase the
///    migration procedure greps for: a document that *has* entries opys cannot
///    read must never be told to add a block it already has. Reporting the whole
///    map rather than reading its well-formed subset is what keeps the three
///    readers honest: a map `verify` blessed but the store could not decompose
///    was destroyed by the next `set-status`;
/// 2. its last entry records the document's current `status`;
/// 3. the keys strictly increase as instants (not as strings). Canonical
///    spelling — RFC3339 UTC, `Z`, exactly six fractional digits — is already
///    part of the shape rule, and is not pedantry: only fixed-width keys have
///    the same lexical and chronological order, which is what the serializer's
///    sorted block relies on, so a `2026-01-01T00:00:00Z` accepted here would be
///    reordered past a `2026-01-01T00:00:00.5Z` by the next unrelated write;
/// 4. `created` and the first key denote the same second, when both exist
///    (`created` carries a local offset, the key is UTC — compare instants).
///
/// Unlike the relation maps, history is never repaired: it cannot be rebuilt
/// from ground truth, so a botched one is reported and fixed by hand.
fn check_history(m: &Frontmatter, id: &str, errors: &mut Vec<String>) {
    // (1a) Present and carrying entries, but not a shape opys can read. Say
    // exactly what is wrong: no append path will write beside this map (and no
    // other path rewrites it) until a human fixes it.
    if let Some(why) = crate::history::botched(m) {
        errors.push(format!(
            "{id}: {why}; opys will not rewrite a lifecycle map it cannot read — \
             repair it by hand (see docs/migration-history.md)"
        ));
        return;
    }
    let entries = crate::history::entries(m);
    if entries.is_empty() {
        errors.push(format!(
            "{id}: missing history (the lifecycle map is written by new/set-status/apply/import; \
             see docs/migration-history.md to seed an existing document)"
        ));
        return;
    }

    // (2) The last entry is the current status.
    if let (Some(status), Some((_, last))) = (m.status(), entries.last()) {
        if last != status {
            errors.push(format!(
                "{id}: history's last entry ({last}) does not match status ({status})"
            ));
        }
    }

    // (3) Keys strictly increase (they are canonical — check 1 guaranteed it).
    let mut prev: Option<(String, OffsetDateTime)> = None;
    for (key, _) in &entries {
        let dt = crate::history::parse(key).expect("is_map checked the key is canonical");
        if let Some((pk, pdt)) = &prev {
            if dt <= *pdt {
                errors.push(format!(
                    "{id}: history keys must strictly increase ('{key}' does not follow '{pk}')"
                ));
            }
        }
        prev = Some((key.clone(), dt));
    }

    // (4) `created` and the first key are the same second. An unparseable
    // `created` is already reported by `check_timestamps`.
    let (first_key, _) = &entries[0];
    if let (Some(created), Some(first)) = (
        m.get_str("created").and_then(crate::history::parse),
        crate::history::parse(first_key),
    ) {
        if created.unix_timestamp() != first.unix_timestamp() {
            errors.push(format!(
                "{id}: 'created' ({}) does not match the first history entry ({first_key})",
                m.get_str("created").unwrap_or_default()
            ));
        }
    }
}

/// Every consecutive pair in the lifecycle map must be an edge the type
/// declares (ADR-0099 decision 1), when it declares any.
///
/// This is what makes the status graph a CI check and not merely a write-time
/// guard: `set-status` refuses an undeclared move, but a document hand-edited
/// into `done`, or one written by an older opys, or one whose type's graph was
/// tightened afterwards, is a *snapshot* verify used to have nothing to say
/// about. The history map turns each move into an event, and an event can be
/// checked against the graph long after it happened.
///
/// Two deliberate silences:
///
/// - a pair whose two statuses are **equal** is skipped. `set-status <id> <its
///   current status>` is a legal no-op that appends an entry, so refusing the
///   pair it writes would make a successful write leave a red corpus — the one
///   thing no check here may do;
/// - the **first** entry is not checked against `default_status`. `import` seeds
///   a document at whatever status its record carried and leaves no marker
///   saying so, so a rule of that shape could only be enforced by calling every
///   imported document a problem. The graph constrains *moves*; where a
///   document entered it is the seeding command's business.
///
/// A botched or absent map yields no entries (`check_history` already reported
/// it), so nothing here fires twice on the same document.
fn check_transitions(
    m: &Frontmatter,
    id: &str,
    tname: &str,
    t: &DocType,
    errors: &mut Vec<String>,
) {
    if t.transitions.is_empty() {
        return;
    }
    let entries = crate::history::entries(m);
    for pair in entries.windows(2) {
        let (from, to) = (&pair[0].1, &pair[1].1);
        if from == to || crate::lifecycle::allows(t, from, to) {
            continue;
        }
        errors.push(format!(
            "{id}: history records {from} -> {to}, which type '{tname}' does not declare as a \
             transition ({})",
            crate::lifecycle::targets_phrase(t, from)
        ));
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
    let Some(Value::Sequence(seq)) = m.get("tags") else {
        return;
    };
    let mut keys: Vec<String> = Vec::new();
    for t in seq {
        let display = tag_display(t);
        match validate_tag(&display) {
            Ok(Some(key)) => {
                if keys.contains(&key) {
                    errors.push(format!("{fid}: tag key '{key}' is set more than once"));
                } else {
                    keys.push(key);
                }
            }
            Ok(None) => {}
            Err(()) => errors.push(format!(
                "{fid}: tag '{display}' must be lowercase kebab-case \
                 (optionally colon-namespaced, or in key=value form)"
            )),
        }
    }
}

/// A colon-namespaced kebab segment: one or more lowercase kebab parts joined
/// by `:` (`osc`, `area:parsing`).
fn is_tag_segment(s: &str) -> bool {
    !s.is_empty() && s.split(':').all(|p| KEBAB_RE.is_match(p))
}

/// Validate a tag's shape. Returns `Ok(Some(key))` for a `key=value` tag (so the
/// caller can enforce key uniqueness), `Ok(None)` for a plain tag, or `Err` if
/// the shape is invalid. Both plain tags and either side of a `key=value` pair
/// are colon-namespaced kebab segments; a tag carries at most one `=`.
fn validate_tag(s: &str) -> std::result::Result<Option<String>, ()> {
    if let Some((key, value)) = s.split_once('=') {
        if value.contains('=') || !is_tag_segment(key) || !is_tag_segment(value) {
            return Err(());
        }
        Ok(Some(key.to_string()))
    } else if is_tag_segment(s) {
        Ok(None)
    } else {
        Err(())
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

/// A `structured` section: its content must conform to the section's `structure`
/// (an mdprism schema). When the section is absent, the `required` check (not
/// this one) is responsible; we only validate present sections.
fn check_structured(d: &Doc, id: &str, sec: &SectionSpec, errors: &mut Vec<String>) {
    let Some(src) = &sec.structure else {
        return;
    };
    if !body::has_section(&d.body, &sec.heading) {
        return;
    }
    // A malformed structure is already reported by config validation.
    let Ok(schema) = crate::mdprism::parse_schema(src) else {
        return;
    };
    let content = body::section(&d.body, &sec.heading);
    for p in schema.validate(&content) {
        errors.push(format!("{id}: section '{}': {p}", sec.heading));
    }
}

/// A `log` section: every bullet carries a UTC timestamp, and the timestamps
/// do not go backwards.
///
/// Non-strict on purpose — equal stamps are fine. Two `opys log` calls inside
/// one second produce the same stamp, and so does a migration that dates a
/// document's undated bullets from its `created`; the property worth defending
/// is that a log reads forwards, not that no two entries share a second. (The
/// `history` map *is* strict, for a different reason: its keys are also its
/// sort order.)
fn check_log(d: &Doc, id: &str, sec: &SectionSpec, errors: &mut Vec<String>) {
    if !body::has_section(&d.body, &sec.heading) {
        return;
    }
    let entries = body::log_entries(&body::section(&d.body, &sec.heading));
    let mut prev: Option<(String, OffsetDateTime)> = None;
    for entry in &entries {
        let Some(ts) = &entry.ts else {
            errors.push(format!(
                "{id}: section '{}': undated log entry {} (every entry starts \
                 with a UTC timestamp — append entries with `opys log`, which \
                 stamps them; see docs/migration-history.md to date existing ones)",
                sec.heading,
                pyrepr(Some(&first_words(&entry.text)))
            ));
            continue;
        };
        let Some(dt) = crate::history::parse(ts) else {
            errors.push(format!(
                "{id}: section '{}': log entry timestamp '{ts}' is not RFC3339",
                sec.heading
            ));
            continue;
        };
        if let Some((pk, pdt)) = &prev {
            if dt < *pdt {
                errors.push(format!(
                    "{id}: section '{}': log entries must not go backwards \
                     ('{ts}' follows '{pk}')",
                    sec.heading
                ));
            }
        }
        prev = Some((ts.clone(), dt));
    }
}

/// The opening of a log entry, for naming it in a problem message.
fn first_words(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    let mut out: String = line.chars().take(40).collect();
    if out.chars().count() < line.chars().count() {
        out.push('…');
    }
    out
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
