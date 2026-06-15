//! The conditional validation-rules engine for the universal config.
//!
//! [`evaluate`] runs a project's `[[rules]]` against one document and returns a
//! problem message for each failed assertion. It is the single place the
//! configured guards are checked; later phases call it from `new`,
//! `set-status`, `import`, and `verify` (today nothing in production calls it —
//! it ships unit-tested, ready to wire in).
//!
//! A rule fires when its `when { type?, status? }` matches the document, then
//! its one assertion (from the closed set) is checked.

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
        if applies(rule, type_name, status) {
            if let Some(msg) = check(prj, rule, fm, body, doc_ids) {
                out.push(msg);
            }
        }
    }
    out
}

/// A rule applies when its (optional) type and status guards both match.
fn applies(rule: &Rule, type_name: &str, status: &str) -> bool {
    rule.when.doc_type.as_deref().is_none_or(|t| t == type_name)
        && rule.when.status.as_deref().is_none_or(|s| s == status)
}

/// Check a rule's single assertion; `Some(msg)` if it fails.
fn check(
    prj: &ProjectConfig,
    rule: &Rule,
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
    None
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

    #[test]
    fn task_requires_feature_link_and_blocked_needs_reason_or_link() {
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

        // blocked with neither reason nor blocker link → require_any fails.
        let blocked = evaluate(&p, "task", "blocked", &fm, "# T\n", &ids(&["FEAT-0001"]));
        assert!(
            blocked.iter().any(|m| m.contains("requires one of")),
            "{blocked:?}"
        );
        // A blocked_by link satisfies it.
        refs::set_in(&mut fm, "blocked_by", &[("FEAT-0002".into(), "B".into())]);
        assert!(evaluate(
            &p,
            "task",
            "blocked",
            &fm,
            "# T\n",
            &ids(&["FEAT-0001", "FEAT-0002"])
        )
        .is_empty());
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
