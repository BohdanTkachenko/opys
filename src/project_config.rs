//! The universal typed-document config (`docs/opys/opys.toml`).
//!
//! This is the data model for the upcoming configurable engine: a project
//! declares its own document **types** (each with statuses, fields, and required
//! sections) plus a list of conditional **validation rules**. Nothing executes
//! against this yet — `opys config validate` parses it and checks it is
//! well-formed, and later phases will make the rest of the tool read it.
//!
//! It deliberately coexists with the legacy `config::Config`/`WorkItemConfig`
//! during the transition; reuses `config::FieldSpec`/`FieldType`/`TestRefCheck`.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use regex::Regex;
use serde::Deserialize;

use crate::config::{
    Config, FieldSpec, FieldType, TestRefCheck, WorkItemConfig, CORE_STATUSES, FEAT_PREFIX,
    WI_CORE_STATUSES, WORK_ITEM_TYPES,
};
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

    /// Synthesize a universal config from the legacy two-file config, so the
    /// engine can run against a project that has not migrated to `opys.toml`.
    /// The `feature` type keeps the `features/` directory; the work-item types
    /// map to `work-items/`. Reproduces today's hardcoded guards as rules.
    pub fn from_legacy(cfg: &Config, wi: Option<&WorkItemConfig>) -> ProjectConfig {
        let mut types = BTreeMap::new();
        let mut rules = Vec::new();

        let mut feature_fields = cfg.fields.clone();
        feature_fields
            .entry("spec".into())
            .or_insert_with(string_field);
        feature_fields
            .entry("wontfix_reason".into())
            .or_insert_with(string_field);
        let mut feature_statuses: Vec<String> =
            CORE_STATUSES.iter().map(|s| s.to_string()).collect();
        feature_statuses.extend(cfg.extra_statuses.iter().cloned());
        types.insert(
            "feature".into(),
            DocType {
                prefix: FEAT_PREFIX.into(),
                dir: Some("features".into()),
                statuses: feature_statuses,
                default_status: "planned".into(),
                terminal_statuses: vec![],
                tags_required: true,
                requires_link: None,
                fields: feature_fields,
                sections: vec![
                    section("Test plan", SectionKind::TestPlan),
                    section("Manual verification", SectionKind::Manual),
                ],
            },
        );
        rules.push(rule_require_field("feature", "wontfix", "wontfix_reason"));
        rules.push(rule_require_checked("feature", "implemented", "Test plan"));

        if let Some(wc) = wi {
            let mut wi_statuses: Vec<String> =
                WI_CORE_STATUSES.iter().map(|s| s.to_string()).collect();
            wi_statuses.extend(wc.extra_statuses.iter().cloned());
            for wt in WORK_ITEM_TYPES {
                let mut fields = wc.fields.clone();
                fields
                    .entry("blocked_reason".into())
                    .or_insert_with(string_field);
                let mut headings = wc.required_sections.clone();
                for extra in wt.extra_required_sections {
                    if !headings.iter().any(|h| h == extra) {
                        headings.push(extra.to_string());
                    }
                }
                let sections = headings
                    .iter()
                    .map(|h| section(h, kind_for_heading(h)))
                    .collect();
                types.insert(
                    wt.name.into(),
                    DocType {
                        prefix: wt.prefix.into(),
                        dir: Some("work-items".into()),
                        statuses: wi_statuses.clone(),
                        default_status: "todo".into(),
                        terminal_statuses: vec!["done".into()],
                        tags_required: false,
                        requires_link: Some(LinkReq {
                            to: "feature".into(),
                            min: 1,
                        }),
                        fields,
                        sections,
                    },
                );
            }
            rules.push(blocked_rule());
        }

        ProjectConfig {
            pad: cfg.pad,
            tests: TestsConfig {
                search_paths: cfg.test_search_paths.clone(),
                reference_check: cfg.test_reference_check,
                name_pattern: cfg.test_name_pattern.clone(),
            },
            report: ReportConfig { parity: cfg.parity },
            types,
            rules,
        }
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

// --- Legacy-adapter helpers -------------------------------------------------

fn string_field() -> FieldSpec {
    FieldSpec {
        field_type: FieldType::String,
        required: false,
        description: None,
        values: vec![],
        pattern: None,
    }
}

fn section(heading: &str, kind: SectionKind) -> SectionSpec {
    SectionSpec {
        heading: heading.to_string(),
        kind,
    }
}

/// Best-effort heading → kind mapping for legacy required-section names.
fn kind_for_heading(h: &str) -> SectionKind {
    match h {
        "Tasks" => SectionKind::Checklist,
        "Progress" => SectionKind::Log,
        "Test plan" => SectionKind::TestPlan,
        "Manual verification" => SectionKind::Manual,
        _ => SectionKind::Prose,
    }
}

fn rule_require_field(doc_type: &str, status: &str, field: &str) -> Rule {
    Rule {
        when: When {
            doc_type: Some(doc_type.into()),
            status: Some(status.into()),
        },
        require_field: Some(field.into()),
        ..Default::default()
    }
}

fn rule_require_checked(doc_type: &str, status: &str, section: &str) -> Rule {
    Rule {
        when: When {
            doc_type: Some(doc_type.into()),
            status: Some(status.into()),
        },
        require_checked_section: Some(section.into()),
        ..Default::default()
    }
}

fn blocked_rule() -> Rule {
    Rule {
        when: When {
            doc_type: None,
            status: Some("blocked".into()),
        },
        require_any: Some(vec![
            AnyTerm {
                field: Some("blocked_reason".into()),
                link: None,
                section: None,
            },
            AnyTerm {
                field: None,
                link: Some("blocked_by".into()),
                section: None,
            },
        ]),
        ..Default::default()
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
    fn legacy_adapter_synthesizes_a_valid_config() {
        let cfg: Config = toml::from_str("pad = 4\ntest_reference_check = \"none\"\n").unwrap();
        let wc: WorkItemConfig =
            toml::from_str("pad = 4\nrequired_sections = [\"Tasks\", \"Progress\"]\n").unwrap();
        let pc = ProjectConfig::from_legacy(&cfg, Some(&wc));
        assert!(pc.validate().is_empty(), "{:?}", pc.validate());
        assert_eq!(pc.types.len(), 4);
        assert_eq!(pc.types["feature"].prefix, "FEAT");
        assert_eq!(pc.types["feature"].dir.as_deref(), Some("features"));
        assert_eq!(pc.types["bug"].dir.as_deref(), Some("work-items"));
        // bug's extra Reproduction section is carried over.
        assert!(pc.types["bug"]
            .sections
            .iter()
            .any(|s| s.heading == "Reproduction"));
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
