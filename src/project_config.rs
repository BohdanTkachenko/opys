//! The universal typed-document config (`docs/opys/opys.toml`) — the sole
//! source of truth for the engine.
//!
//! A project declares its own document **types** (each with a prefix, dir,
//! statuses, fields, and required sections) plus a list of conditional
//! **validation rules**. `Project::open` loads this; every command reads it, and
//! `verify` enforces it via [`crate::rules`]. Reuses
//! `config::FieldSpec`/`FieldType`/`TestRefCheck`.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use regex::Regex;
use serde::Deserialize;

use crate::config::{FieldSpec, FieldType, TestRefCheck};
use crate::error::{usage, OpysError, Result};
use crate::refs;

fn default_pad() -> usize {
    4
}
fn default_search_paths() -> Vec<String> {
    vec!["src".to_string(), "tests".to_string()]
}
fn default_min() -> usize {
    1
}

/// Directory (under the inventory base) for a type that declares no explicit
/// `dir` — by default every type's docs live together here.
pub const DEFAULT_DOC_DIR: &str = "items";

/// A built-in section behavior a type's section opts into. The validator and
/// scaffold for each kind are compiled code (closed set, not extensible from
/// config) — this is the guardrail that keeps the engine opinionated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SectionKind {
    Prose,
    Log,
    Checklist,
    TestPlan,
    Manual,
}

impl SectionKind {
    /// Whether "≥1 checked item" is meaningful for this kind.
    pub fn is_checkable(self) -> bool {
        matches!(self, SectionKind::Checklist | SectionKind::TestPlan)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SectionSpec {
    pub heading: String,
    pub kind: SectionKind,
    /// Whether the section must be present (verify enforces it; `new` scaffolds it).
    #[serde(default)]
    pub required: bool,
}

/// `requires_link = { to = "feature", min = 1 }` — a type must reference ≥`min`
/// docs of type `to`.
#[derive(Debug, Clone, Deserialize)]
pub struct LinkReq {
    pub to: String,
    #[serde(default = "default_min")]
    pub min: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DocType {
    pub prefix: String,
    /// Directory under the inventory base holding this type's files (defaults to
    /// the type name). The legacy adapter sets `features`/`work-items`.
    #[serde(default)]
    pub dir: Option<String>,
    #[serde(default)]
    pub statuses: Vec<String>,
    #[serde(default)]
    pub default_status: String,
    #[serde(default)]
    pub terminal_statuses: Vec<String>,
    #[serde(default)]
    pub tags_required: bool,
    #[serde(default)]
    pub requires_link: Option<LinkReq>,
    #[serde(default)]
    pub fields: BTreeMap<String, FieldSpec>,
    #[serde(default)]
    pub sections: Vec<SectionSpec>,
}

impl DocType {
    /// The directory under the base holding this type's files (its `dir`, or the
    /// shared default).
    pub fn resolved_dir(&self) -> &str {
        self.dir.as_deref().unwrap_or(DEFAULT_DOC_DIR)
    }
}

/// A rule's match guard. Both fields optional: omitting both means "always".
#[derive(Debug, Clone, Default, Deserialize)]
pub struct When {
    #[serde(default, rename = "type")]
    pub doc_type: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

/// One term of a `require_any` (exactly one of the three is set).
#[derive(Debug, Clone, Deserialize)]
pub struct AnyTerm {
    #[serde(default)]
    pub field: Option<String>,
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub section: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FieldMatch {
    pub field: String,
    pub pattern: String,
}

/// A conditional validation rule: a `when` guard plus exactly one assertion
/// from the closed set below.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Rule {
    #[serde(default)]
    pub when: When,
    #[serde(default)]
    pub require_field: Option<String>,
    #[serde(default)]
    pub require_section: Option<String>,
    #[serde(default)]
    pub require_checked_section: Option<String>,
    #[serde(default)]
    pub require_link: Option<LinkReq>,
    #[serde(default)]
    pub require_any: Option<Vec<AnyTerm>>,
    #[serde(default)]
    pub field_matches: Option<FieldMatch>,
}

impl Rule {
    /// How many assertions this rule sets (must be exactly one).
    fn assertion_count(&self) -> usize {
        [
            self.require_field.is_some(),
            self.require_section.is_some(),
            self.require_checked_section.is_some(),
            self.require_link.is_some(),
            self.require_any.is_some(),
            self.field_matches.is_some(),
        ]
        .iter()
        .filter(|b| **b)
        .count()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TestsConfig {
    #[serde(default = "default_search_paths")]
    pub search_paths: Vec<String>,
    #[serde(default)]
    pub reference_check: TestRefCheck,
    #[serde(default)]
    pub name_pattern: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReportConfig {
    #[serde(default)]
    pub parity: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectConfig {
    #[serde(default = "default_pad")]
    pub pad: usize,
    #[serde(default)]
    pub tests: TestsConfig,
    #[serde(default)]
    pub report: ReportConfig,
    #[serde(default)]
    pub types: BTreeMap<String, DocType>,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

impl ProjectConfig {
    /// Load `opys.toml`, or a usage error pointing at `config init` when absent.
    pub fn load(path: &Path) -> Result<ProjectConfig> {
        if !path.exists() {
            return Err(usage(format!(
                "no opys.toml at {} — run `opys config init`",
                path.display()
            )));
        }
        let text = std::fs::read_to_string(path)?;
        toml::from_str(&text).map_err(|source| OpysError::Toml {
            path: path.to_path_buf(),
            source,
        })
    }

    /// The distinct directories (under the base) that hold documents — the set
    /// generic discovery scans. Multiple types may share a dir (assigned by id
    /// prefix at load).
    pub fn doc_dirs(&self) -> Vec<&str> {
        let mut dirs: Vec<&str> = self.types.values().map(DocType::resolved_dir).collect();
        dirs.sort_unstable();
        dirs.dedup();
        dirs
    }

    /// The directory holding the type whose prefix matches `id` (the default dir
    /// when the prefix matches no type).
    pub fn dir_for_id(&self, id: &str) -> &str {
        self.type_name_for_id(id)
            .and_then(|n| self.types.get(n))
            .map(DocType::resolved_dir)
            .unwrap_or(DEFAULT_DOC_DIR)
    }

    /// The name of the type whose prefix matches `id`'s prefix, if any. (A doc's
    /// type is derived from its ID prefix — the ID is the single source of truth.)
    pub fn type_name_for_id(&self, id: &str) -> Option<&str> {
        let prefix = id.split_once('-')?.0;
        self.types
            .iter()
            .find(|(_, t)| t.prefix == prefix)
            .map(|(name, _)| name.as_str())
    }

    /// Check the config is well-formed, returning all problems (empty == OK).
    /// These are content problems (verify-style), not hard errors.
    pub fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();
        if self.types.is_empty() {
            errs.push("no document types defined ([types.<name>])".into());
        }

        let prefix_re = Regex::new(r"^[A-Z][A-Z0-9]*$").unwrap();
        let type_names: HashSet<&str> = self.types.keys().map(String::as_str).collect();
        let mut seen_prefix: BTreeMap<&str, &str> = BTreeMap::new();

        for (name, t) in &self.types {
            if !prefix_re.is_match(&t.prefix) {
                errs.push(format!(
                    "type '{name}': prefix '{}' must match ^[A-Z][A-Z0-9]*$",
                    t.prefix
                ));
            }
            if let Some(prev) = seen_prefix.insert(&t.prefix, name) {
                errs.push(format!(
                    "type '{name}': prefix '{}' already used by type '{prev}'",
                    t.prefix
                ));
            }
            if t.statuses.is_empty() {
                errs.push(format!("type '{name}': statuses must be non-empty"));
            }
            if t.default_status.is_empty() {
                errs.push(format!("type '{name}': default_status is required"));
            } else if !t.statuses.contains(&t.default_status) {
                errs.push(format!(
                    "type '{name}': default_status '{}' not in statuses",
                    t.default_status
                ));
            }
            for s in &t.terminal_statuses {
                if !t.statuses.contains(s) {
                    errs.push(format!(
                        "type '{name}': terminal_status '{s}' not in statuses"
                    ));
                }
            }
            if let Some(lr) = &t.requires_link {
                if !type_names.contains(lr.to.as_str()) {
                    errs.push(format!(
                        "type '{name}': requires_link.to '{}' is not a defined type",
                        lr.to
                    ));
                }
            }
            for (fname, spec) in &t.fields {
                if spec.field_type == FieldType::Enum && spec.values.is_empty() {
                    errs.push(format!(
                        "type '{name}' field '{fname}': enum declares no values"
                    ));
                }
                if let Some(p) = &spec.pattern {
                    if Regex::new(p).is_err() {
                        errs.push(format!(
                            "type '{name}' field '{fname}': pattern is not a valid regex"
                        ));
                    }
                }
            }
            let mut seen_section: HashSet<&str> = HashSet::new();
            for sec in &t.sections {
                if !seen_section.insert(sec.heading.as_str()) {
                    errs.push(format!(
                        "type '{name}': duplicate section heading '{}'",
                        sec.heading
                    ));
                }
            }
        }

        if self.tests.reference_check == TestRefCheck::Extract {
            match &self.tests.name_pattern {
                None => errs
                    .push("tests.reference_check = \"extract\" requires tests.name_pattern".into()),
                Some(p) if Regex::new(p).is_err() => {
                    errs.push("tests.name_pattern is not a valid regex".into())
                }
                _ => {}
            }
        }

        for (i, rule) in self.rules.iter().enumerate() {
            self.validate_rule(i + 1, rule, &type_names, &mut errs);
        }
        errs
    }

    fn validate_rule(&self, n: usize, r: &Rule, types: &HashSet<&str>, errs: &mut Vec<String>) {
        let tag = format!("rule #{n}");
        match r.assertion_count() {
            1 => {}
            0 => errs.push(format!("{tag}: has no assertion")),
            _ => errs.push(format!("{tag}: has more than one assertion")),
        }

        // `when` guard resolves.
        if let Some(t) = &r.when.doc_type {
            if !types.contains(t.as_str()) {
                errs.push(format!("{tag}: when.type '{t}' is not a defined type"));
            } else if let Some(s) = &r.when.status {
                if !self.types[t].statuses.contains(s) {
                    errs.push(format!(
                        "{tag}: when.status '{s}' is not a status of type '{t}'"
                    ));
                }
            }
        }

        // `require_link.to` is a type.
        if let Some(lr) = &r.require_link {
            if !types.contains(lr.to.as_str()) {
                errs.push(format!(
                    "{tag}: require_link.to '{}' is not a defined type",
                    lr.to
                ));
            }
        }

        // `require_any` terms: exactly one key each; a `link` is a relation field.
        if let Some(terms) = &r.require_any {
            if terms.is_empty() {
                errs.push(format!("{tag}: require_any is empty"));
            }
            for term in terms {
                let count = [
                    term.field.is_some(),
                    term.link.is_some(),
                    term.section.is_some(),
                ]
                .iter()
                .filter(|b| **b)
                .count();
                if count != 1 {
                    errs.push(format!(
                        "{tag}: each require_any term needs exactly one of field/link/section"
                    ));
                }
                if let Some(l) = &term.link {
                    if !refs::RELATION_FIELDS.contains(&l.as_str()) {
                        errs.push(format!(
                            "{tag}: require_any link '{l}' is not a relation field (references/blocked_by/blocks)"
                        ));
                    }
                }
            }
        }

        // Field/section assertions are resolvable against the named type.
        if let Some(t) = &r.when.doc_type {
            if let Some(dt) = self.types.get(t) {
                if let Some(f) = &r.require_field {
                    if !dt.fields.contains_key(f) {
                        errs.push(format!(
                            "{tag}: require_field '{f}' is not a field of type '{t}'"
                        ));
                    }
                }
                if let Some(sec) = &r.require_section {
                    if !dt.sections.iter().any(|s| &s.heading == sec) {
                        errs.push(format!(
                            "{tag}: require_section '{sec}' is not a section of type '{t}'"
                        ));
                    }
                }
                if let Some(sec) = &r.require_checked_section {
                    match dt.sections.iter().find(|s| &s.heading == sec) {
                        None => errs.push(format!(
                            "{tag}: require_checked_section '{sec}' is not a section of type '{t}'"
                        )),
                        Some(s) if !s.kind.is_checkable() => errs.push(format!(
                            "{tag}: require_checked_section '{sec}' targets a non-checklist section"
                        )),
                        _ => {}
                    }
                }
            }
        }

        // A `field_matches.pattern` must always compile.
        if let Some(fm) = &r.field_matches {
            if Regex::new(&fm.pattern).is_err() {
                errs.push(format!("{tag}: field_matches.pattern is not a valid regex"));
            }
            if let Some(t) = &r.when.doc_type {
                if let Some(dt) = self.types.get(t) {
                    if !dt.fields.contains_key(&fm.field) {
                        errs.push(format!(
                            "{tag}: field_matches.field '{}' is not a field of type '{t}'",
                            fm.field
                        ));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templates::DEFAULT_OPYS_CONFIG;

    #[test]
    fn default_config_validates_clean() {
        let cfg: ProjectConfig = toml::from_str(DEFAULT_OPYS_CONFIG).unwrap();
        let problems = cfg.validate();
        assert!(
            problems.is_empty(),
            "default config has problems: {problems:?}"
        );
        assert_eq!(cfg.types.len(), 4);
    }

    #[test]
    fn flags_bad_default_status_and_duplicate_prefix_and_unknown_rule_type() {
        let cfg: ProjectConfig = toml::from_str(
            r#"
[types.feature]
prefix = "FEAT"
statuses = ["planned"]
default_status = "nope"

[types.bug]
prefix = "FEAT"
statuses = ["todo"]
default_status = "todo"

[[rules]]
when = { type = "ghost" }
require_field = "x"
"#,
        )
        .unwrap();
        let problems = cfg.validate();
        let joined = problems.join("\n");
        assert!(
            joined.contains("default_status 'nope' not in statuses"),
            "{joined}"
        );
        assert!(joined.contains("already used by type"), "{joined}");
        assert!(
            joined.contains("when.type 'ghost' is not a defined type"),
            "{joined}"
        );
    }
}
