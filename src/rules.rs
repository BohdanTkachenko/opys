//! The conditional validation-rules engine for the universal config.
//!
//! [`evaluate`] runs a project's `[[rules]]` against one document and returns a
//! problem message for each failed assertion. It is the single place the
//! configured guards are checked, and it is called from every write point
//! (`new`, `set-status`, `import`, `retire`) as well as from `verify`.
//!
//! A rule fires when its `when { type?, status?, tag? }` matches the document,
//! then its one assertion (from the closed set) is checked.

use std::collections::HashSet;

use regex::Regex;
use serde_norway::Value;

use crate::frontmatter::Frontmatter;
use crate::project_config::{AnyTerm, ProjectConfig, Rule};
use crate::{body, refs};

/// Evaluate every applicable rule against one document. `doc_ids` is the set of
/// ids a link may resolve to (the live docs), exactly as `verify` builds it.
pub fn evaluate(
    prj: &ProjectConfig,
    type_name: &str,
    status: &str,
    fm: &Frontmatter,
    body: &str,
    doc_ids: &HashSet<String>,
) -> Vec<String> {
    let mut out = Vec::new();
    // The type-level `requires_link` shorthand (sugar for an always-on
    // require_link rule on this type).
    if let Some(dt) = prj.types.get(type_name) {
        if let Some(lr) = &dt.requires_link {
            if resolved_links(prj, fm, &lr.to, doc_ids) < lr.min {
                out.push(format!(
                    "must reference at least {} doc(s) of type '{}'",
                    lr.min, lr.to
                ));
            }
        }
    }
    for rule in &prj.rules {
        if applies(rule, type_name, status, fm) {
            if let Some(msg) = check(prj, rule, status, fm, body, doc_ids) {
                out.push(msg);
            }
        }
    }
    out
}

/// A rule applies when its (optional) type, status, and tag guards all match.
fn applies(rule: &Rule, type_name: &str, status: &str, fm: &Frontmatter) -> bool {
    rule.when.doc_type.as_deref().is_none_or(|t| t == type_name)
        && rule.when.status.as_deref().is_none_or(|s| s == status)
        && rule.when.tag.as_deref().is_none_or(|t| fm.has_tag(t))
}

/// Check a rule's single assertion; `Some(msg)` if it fails. `status` is the
/// status the document is being evaluated *at* — the one it is moving to at a
/// write point, the one it already holds at `verify` time — which
/// `require_log_since_status` needs and the rest ignore.
fn check(
    prj: &ProjectConfig,
    rule: &Rule,
    status: &str,
    fm: &Frontmatter,
    body: &str,
    doc_ids: &HashSet<String>,
) -> Option<String> {
    if let Some(f) = &rule.require_field {
        if !field_present(fm, f) {
            return Some(format!("field '{f}' is required"));
        }
    }
    if let Some(m) = &rule.field_matches {
        // Validate the value only when present (presence is require_field's job).
        if let Some(s) = fm.get_str(&m.field) {
            if Regex::new(&m.pattern)
                .map(|re| !re.is_match(s))
                .unwrap_or(false)
            {
                return Some(format!("field '{}' must match /{}/", m.field, m.pattern));
            }
        }
    }
    if let Some(h) = &rule.require_section {
        if !body::has_section(body, h) {
            return Some(format!("missing required '## {h}' section"));
        }
    }
    if let Some(h) = &rule.require_checked_section {
        if !body::checklist_items(body, h).iter().any(|i| i.checked) {
            return Some(format!("'## {h}' needs at least one checked item"));
        }
    }
    if let Some(lr) = &rule.require_link {
        if resolved_links(prj, fm, &lr.to, doc_ids) < lr.min {
            return Some(format!(
                "must reference at least {} doc(s) of type '{}'",
                lr.min, lr.to
            ));
        }
    }
    if let Some(terms) = &rule.require_any {
        if !terms.iter().any(|t| any_term_holds(t, fm, body)) {
            return Some(format!("requires one of: {}", describe_any(terms)));
        }
    }
    if let Some(h) = &rule.require_log_since_status {
        if let Some(since) = since_instant(fm, status) {
            let fresh = body::log_entries(&body::section(body, h))
                .iter()
                .filter_map(|e| e.ts.as_deref().and_then(crate::history::parse))
                // Seconds, because a history key carries six fractional digits
                // and a log stamp carries none: `opys log` followed by
                // `set-status` inside one second must pass.
                .any(|t| t.unix_timestamp() >= since.unix_timestamp());
            if !fresh {
                return Some(format!(
                    "'## {h}' has no log entry since the status last changed \
                     (append one with `opys log`)"
                ));
            }
        }
    }
    None
}

/// The instant a `require_log_since_status` entry must not predate: the *last*
/// `history` entry whose status differs from `status`, falling back to the
/// first key when every entry holds it.
///
/// That definition, rather than "the last key", is what makes the two call
/// sites agree. `set_status::core` evaluates the rules *before* appending the
/// new entry, so at write time the map ends at the status being left and the
/// last differing entry is that one; at `verify` time the new entry is there
/// and "last differing" skips it, landing on the same key. A
/// `review → in-progress → review` bounce moves the answer forward to the
/// `in-progress` entry, so each visit needs its own fresh entry.
///
/// `None` — no history at all — passes the assertion. A document without a
/// lifecycle map is already reported by `verify` under its own name; failing it
/// twice for one cause is noise.
fn since_instant(fm: &Frontmatter, status: &str) -> Option<time::OffsetDateTime> {
    let entries = crate::history::entries(fm);
    entries
        .iter()
        .rev()
        .find(|(_, s)| s != status)
        .or_else(|| entries.first())
        .and_then(|(k, _)| crate::history::parse(k))
}

/// A field is "set" when present and not null / not an empty string.
fn field_present(fm: &Frontmatter, name: &str) -> bool {
    match fm.get(name) {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(_) => true,
    }
}

/// Count references (in the `references` map) to live docs of `to_type`.
fn resolved_links(
    prj: &ProjectConfig,
    fm: &Frontmatter,
    to_type: &str,
    doc_ids: &HashSet<String>,
) -> usize {
    match prj.types.get(to_type) {
        Some(dt) => refs::ids_with_prefix(fm, &dt.prefix)
            .iter()
            .filter(|id| doc_ids.contains(*id))
            .count(),
        None => 0,
    }
}

/// One `require_any` term holds: a set field, a non-empty relation link, or a
/// present section.
fn any_term_holds(term: &AnyTerm, fm: &Frontmatter, body: &str) -> bool {
    if let Some(f) = &term.field {
        return field_present(fm, f);
    }
    if let Some(l) = &term.link {
        return !refs::parse_in(fm, l).is_empty();
    }
    if let Some(s) = &term.section {
        return body::has_section(body, s);
    }
    if let Some(t) = &term.tag {
        return fm.has_tag(t);
    }
    false
}

fn describe_any(terms: &[AnyTerm]) -> String {
    terms
        .iter()
        .map(|t| {
            if let Some(f) = &t.field {
                format!("field '{f}'")
            } else if let Some(l) = &t.link {
                format!("link '{l}'")
            } else if let Some(s) = &t.section {
                format!("section '{s}'")
            } else if let Some(tag) = &t.tag {
                format!("tag '{tag}'")
            } else {
                "?".to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_config::ProjectConfig;
    use crate::templates::DEFAULT_OPYS_CONFIG;

    fn cfg() -> ProjectConfig {
        toml::from_str(DEFAULT_OPYS_CONFIG).unwrap()
    }
    fn ids(list: &[&str]) -> HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn wontfix_requires_reason() {
        let p = cfg();
        let mut fm = Frontmatter::new();
        // No wontfix_reason → fails.
        let problems = evaluate(&p, "feature", "wontfix", &fm, "# F\n", &ids(&[]));
        assert!(
            problems.iter().any(|m| m.contains("wontfix_reason")),
            "{problems:?}"
        );
        // With a reason → clean.
        fm.set_str("wontfix_reason", "scope cut");
        assert!(evaluate(&p, "feature", "wontfix", &fm, "# F\n", &ids(&[])).is_empty());
    }

    #[test]
    fn implemented_requires_checked_test_plan() {
        let p = cfg();
        let fm = Frontmatter::new();
        let bare = "# F\n\n## Test plan\n- [ ] todo\n";
        assert!(evaluate(&p, "feature", "implemented", &fm, bare, &ids(&[]))
            .iter()
            .any(|m| m.contains("Test plan")));
        let done = "# F\n\n## Test plan\n- [x] covered — `m::t`\n";
        assert!(evaluate(&p, "feature", "implemented", &fm, done, &ids(&[])).is_empty());
    }

    /// The shipped `task` type's one rule. There is no blocked rule to pair it
    /// with any more: blocked is derived from the live `blocked_by` entries
    /// (ADR-0099 decision 3), so there is no status left for a rule to fire on.
    /// The `require_any` *assertion* is untouched, and still covered by
    /// `require_any_accepts_a_tag_term` below.
    #[test]
    fn task_requires_a_feature_link() {
        let p = cfg();
        // No references → the require_link rule fails.
        let mut fm = Frontmatter::new();
        let problems = evaluate(&p, "task", "todo", &fm, "# T\n", &ids(&[]));
        assert!(
            problems.iter().any(|m| m.contains("type 'feature'")),
            "{problems:?}"
        );

        // Link a live feature → require_link satisfied.
        refs::set(&mut fm, &[("FEAT-0001".into(), "F".into())]);
        assert!(evaluate(&p, "task", "todo", &fm, "# T\n", &ids(&["FEAT-0001"])).is_empty());

        // A blocker link is a relation, not a rule trigger: it adds nothing to
        // check at any status.
        refs::set_in(&mut fm, "blocked_by", &[("FEAT-0002".into(), "B".into())]);
        assert!(evaluate(
            &p,
            "task",
            "in-progress",
            &fm,
            "# T\n",
            &ids(&["FEAT-0001", "FEAT-0002"])
        )
        .is_empty());
    }

    #[test]
    fn when_tag_gates_a_rule_by_tag_key() {
        // The rule fires only for docs carrying an `area` tag (exact or key),
        // and then requires the `owner` field.
        let p: ProjectConfig = toml::from_str(
            r#"
[types.feature]
prefix = "FEAT"
statuses = ["planned"]
default_status = "planned"
[types.feature.fields.owner]
type = "string"

[[rules]]
when = { tag = "area" }
require_field = "owner"
"#,
        )
        .unwrap();
        assert!(p.validate().is_empty(), "{:?}", p.validate());

        // No `area` tag → rule does not fire, even without `owner`.
        let mut fm = Frontmatter::new();
        fm.set_tags(&["osc".into()]);
        assert!(evaluate(&p, "feature", "planned", &fm, "# F\n", &ids(&[])).is_empty());

        // `area:parsing` matches the `area` key → rule fires, `owner` missing.
        fm.set_tags(&["area:parsing".into()]);
        let problems = evaluate(&p, "feature", "planned", &fm, "# F\n", &ids(&[]));
        assert!(problems.iter().any(|m| m.contains("owner")), "{problems:?}");

        // Add `owner` → clean.
        fm.set_str("owner", "dan");
        assert!(evaluate(&p, "feature", "planned", &fm, "# F\n", &ids(&[])).is_empty());
    }

    #[test]
    fn require_any_accepts_a_tag_term() {
        let p: ProjectConfig = toml::from_str(
            r#"
[types.feature]
prefix = "FEAT"
statuses = ["planned"]
default_status = "planned"

[[rules]]
when = { status = "planned" }
require_any = [ { tag = "area" }, { section = "Notes" } ]
"#,
        )
        .unwrap();
        assert!(p.validate().is_empty(), "{:?}", p.validate());

        // Neither a matching tag nor the section → fails.
        let mut fm = Frontmatter::new();
        fm.set_tags(&["osc".into()]);
        let problems = evaluate(&p, "feature", "planned", &fm, "# F\n", &ids(&[]));
        assert!(
            problems.iter().any(|m| m.contains("requires one of")),
            "{problems:?}"
        );

        // An `area=cli` tag satisfies the tag term.
        fm.set_tags(&["area=cli".into()]);
        assert!(evaluate(&p, "feature", "planned", &fm, "# F\n", &ids(&[])).is_empty());
    }

    /// A `history` map from `(key, status)` pairs, exactly as the file holds it.
    fn with_history(pairs: &[(&str, &str)]) -> Frontmatter {
        let mut fm = Frontmatter::new();
        for (key, status) in pairs {
            crate::history::append(&mut fm, key, status);
        }
        fm
    }

    fn progress(stamps: &[&str]) -> String {
        let mut body = "# T\n\n## Progress\n".to_string();
        for s in stamps {
            body.push_str(&format!("- {s} did a thing\n"));
        }
        body
    }

    #[test]
    fn review_needs_a_log_entry_since_the_status_last_changed() {
        let p = cfg();
        let live = ids(&["FEAT-0001"]);
        let mut fm = with_history(&[
            ("2026-01-01T00:00:00.000000Z", "todo"),
            ("2026-01-02T00:00:00.000000Z", "in-progress"),
        ]);
        refs::set(&mut fm, &[("FEAT-0001".into(), "F".into())]);

        // Write time: the `review` entry is not appended yet, so `since` is the
        // in-progress key. An entry from before it does not count.
        let stale = progress(&["2026-01-01T12:00:00Z"]);
        assert!(
            evaluate(&p, "task", "review", &fm, &stale, &live)
                .iter()
                .any(|m| m.contains("no log entry since")),
            "{:?}",
            evaluate(&p, "task", "review", &fm, &stale, &live)
        );
        // An entry stamped in the same second as the transition is fresh
        // enough: the comparison is at seconds granularity.
        let fresh = progress(&["2026-01-01T12:00:00Z", "2026-01-02T00:00:00Z"]);
        assert!(evaluate(&p, "task", "review", &fm, &fresh, &live).is_empty());

        // Verify time: the same document, now carrying its `review` entry.
        // "The last entry whose status differs" lands on the same key, so the
        // verdict does not change.
        let mut after = fm.clone();
        crate::history::append(&mut after, "2026-01-02T00:00:01.000000Z", "review");
        assert!(evaluate(&p, "task", "review", &after, &fresh, &live).is_empty());
        assert!(!evaluate(&p, "task", "review", &after, &stale, &live).is_empty());
    }

    #[test]
    fn a_review_bounce_needs_a_fresh_entry_each_time() {
        let p = cfg();
        let live = ids(&["FEAT-0001"]);
        let mut fm = with_history(&[
            ("2026-01-01T00:00:00.000000Z", "todo"),
            ("2026-01-02T00:00:00.000000Z", "review"),
            ("2026-01-03T00:00:00.000000Z", "in-progress"),
        ]);
        refs::set(&mut fm, &[("FEAT-0001".into(), "F".into())]);
        // The entry that satisfied the first visit to `review` is stale for the
        // second: `since` has moved on to the in-progress key.
        let first_visit = progress(&["2026-01-02T00:00:00Z"]);
        assert!(!evaluate(&p, "task", "review", &fm, &first_visit, &live).is_empty());
        let second_visit = progress(&["2026-01-02T00:00:00Z", "2026-01-03T09:00:00Z"]);
        assert!(evaluate(&p, "task", "review", &fm, &second_visit, &live).is_empty());
    }

    #[test]
    fn require_log_since_status_falls_back_to_the_first_key_and_passes_without_history() {
        let p = cfg();
        let live = ids(&["FEAT-0001"]);
        // Created straight into `review`: no differing entry, so the first key
        // is the boundary.
        let mut fm = with_history(&[("2026-01-02T00:00:00.000000Z", "review")]);
        refs::set(&mut fm, &[("FEAT-0001".into(), "F".into())]);
        assert!(!evaluate(&p, "task", "review", &fm, &progress(&[]), &live).is_empty());
        let logged = progress(&["2026-01-02T00:00:00Z"]);
        assert!(evaluate(&p, "task", "review", &fm, &logged, &live).is_empty());

        // No history at all: `verify` reports that under its own name, so the
        // rule stays quiet rather than failing twice for one cause.
        let mut bare = Frontmatter::new();
        refs::set(&mut bare, &[("FEAT-0001".into(), "F".into())]);
        assert!(evaluate(&p, "task", "review", &bare, &progress(&[]), &live).is_empty());
    }

    #[test]
    fn rules_only_fire_for_their_type() {
        let p = cfg();
        let fm = Frontmatter::new();
        // The feature wontfix rule must not fire for a task in status "wontfix"
        // (tasks don't even have that status, but the engine just checks `when`).
        let problems = evaluate(&p, "task", "wontfix", &fm, "# T\n", &ids(&["FEAT-0001"]));
        assert!(
            !problems.iter().any(|m| m.contains("wontfix_reason")),
            "{problems:?}"
        );
    }
}
