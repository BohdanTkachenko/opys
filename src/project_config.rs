//! The universal typed-document config (`opys.toml`) — the sole
//! source of truth for the engine.
//!
//! A project declares its own document **types** (each with a prefix, dir,
//! statuses, fields, and required sections) plus a list of conditional
//! **validation rules**. `Project::open` loads this; every command reads it, and
//! `verify` enforces it via [`crate::rules`]. Reuses
//! `config::FieldSpec`/`FieldType`/`TestRefCheck`.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::Deserialize;

use crate::config::{FieldSpec, FieldType};
use crate::error::{usage, OpysError, Result};
use crate::palette::{parse_color, PaletteEntry};
use crate::refs;

fn default_pad() -> usize {
    4
}
fn default_base() -> String {
    DEFAULT_BASE.to_string()
}
fn default_roots() -> Vec<String> {
    vec![".".to_string()]
}
fn default_min() -> usize {
    1
}
fn default_layout_path() -> String {
    DEFAULT_LAYOUT_PATH.to_string()
}

/// Placeholder names in a layout template that aren't one of `type`/`status`/`id`.
fn unknown_placeholders(template: &str) -> Vec<&str> {
    let known = ["type", "status", "id"];
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        rest = &rest[open + 1..];
        if let Some(close) = rest.find('}') {
            let name = &rest[..close];
            if !known.contains(&name) && !out.contains(&name) {
                out.push(name);
            }
            rest = &rest[close + 1..];
        } else {
            break;
        }
    }
    out
}

/// Inventory base directory (the documents and `_retired.txt`), relative to the
/// project root that holds `opys.toml`; the config `base` default.
pub const DEFAULT_BASE: &str = "opys";

/// Directory (under the inventory base) for a type that declares no explicit
/// `dir`. Empty by default → documents live flat at the base.
pub const DEFAULT_DOC_DIR: &str = "";

/// Default file-path template (relative to the base). Both the `{type}` and
/// `{status}` segments are empty by default, so this collapses to a flat
/// `PREFIX-NNNN.md` at the base.
pub const DEFAULT_LAYOUT_PATH: &str = "{type}/{status}/{id}.md";

/// The on-disk layout: a single path template rendered per document. The
/// `{type}` placeholder resolves to the type's `dir`, `{status}` to the type's
/// `status_dirs[status]` (both empty by default), and `{id}` to `PREFIX-NNNN`.
/// Empty segments collapse, so the order of segments is freely configurable.
#[derive(Debug, Clone, Deserialize)]
pub struct Layout {
    #[serde(default = "default_layout_path")]
    pub path: String,
}

impl Default for Layout {
    fn default() -> Self {
        Layout {
            path: default_layout_path(),
        }
    }
}

/// A built-in section behavior a type's section opts into. The validator and
/// scaffold for each kind are compiled code (closed set, not extensible from
/// config) — this is the guardrail that keeps the engine opinionated. The
/// `structured` kind is the configurable one: its item format is declared by
/// the section's `[[parts]]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SectionKind {
    Prose,
    Log,
    Checklist,
    Structured,
}

impl SectionKind {
    /// Whether "≥1 checked item" is meaningful for this kind.
    pub fn is_checkable(self) -> bool {
        matches!(self, SectionKind::Checklist)
    }
}

/// How a `structured` section's part is written under an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PartForm {
    /// A single `- <Label>: <value>` bullet.
    #[default]
    Value,
    /// A `- <Label>:` bullet followed by a numbered list.
    Ordered,
}

/// One named part of a `structured` section's item (e.g. `Setup`, `Steps`,
/// `Expect`). The set of parts *is* the configurable item format.
#[derive(Debug, Clone, Deserialize)]
pub struct PartSpec {
    /// The bullet label, written as `- <label>: …` under each item.
    pub label: String,
    #[serde(default)]
    pub form: PartForm,
    /// Whether every item must carry this part (verify enforces it).
    #[serde(default)]
    pub required: bool,
}

/// Which lines of a section a [`SectionCheck`] validates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckScope {
    /// Every line in the section; lines not matching `pattern` are skipped.
    #[default]
    All,
    /// Only checked checklist items; a checked item with no `pattern` match is
    /// itself an error.
    Checked,
}

/// A universal, config-driven content check attached to a section. `pattern`
/// parses a line into named capture groups; `file` (a group name) and/or
/// `must_match` (a regex built from `${group}` substitutions) then assert the
/// captured reference points at something real.
#[derive(Debug, Clone, Deserialize)]
pub struct SectionCheck {
    /// Regex parsing one line into named groups. A line that does not match is
    /// prose (skipped); a match is validated.
    pub pattern: String,
    /// Optional capture-group name holding a file path to open (resolved against
    /// `roots`). Its presence is what makes the check file-scoped vs corpus-wide.
    #[serde(default)]
    pub file: Option<String>,
    /// Directories the `file` path / corpus resolve against (project-root
    /// relative). Defaults to `["."]`.
    #[serde(default = "default_roots")]
    pub roots: Vec<String>,
    /// Optional regex that must match in the opened file (or, when `file` is
    /// unset, in the concatenated corpus under `roots`). `${group}` is replaced
    /// by the regex-escaped capture from `pattern`.
    #[serde(default)]
    pub must_match: Option<String>,
    /// Which lines to validate. Defaults to `all`.
    #[serde(default)]
    pub scope: CheckScope,
    /// Optional custom failure message; `${group}` is replaced by the raw
    /// capture from `pattern`.
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SectionSpec {
    pub heading: String,
    pub kind: SectionKind,
    /// Whether the section must be present (verify enforces it; `new` scaffolds it).
    #[serde(default)]
    pub required: bool,
    /// Universal content checks run over this section at `verify` time.
    #[serde(default)]
    pub checks: Vec<SectionCheck>,
    /// The item format for a `structured` section: its named parts. Empty (and
    /// rejected by validation) for every other kind.
    #[serde(default)]
    pub parts: Vec<PartSpec>,
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
    /// Per-status subdirectory (the `{status}` layout segment). A status absent
    /// from the map contributes an empty segment. E.g. `archived = "_archived"`.
    #[serde(default)]
    pub status_dirs: BTreeMap<String, String>,
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
    /// The `{type}` layout segment for this type: its `dir`, or the default
    /// (empty → flat at the base).
    pub fn resolved_dir(&self) -> &str {
        self.dir.as_deref().unwrap_or(DEFAULT_DOC_DIR)
    }

    /// The `{status}` layout segment for the given status (empty if unmapped).
    pub fn status_dir(&self, status: &str) -> &str {
        self.status_dirs.get(status).map_or("", String::as_str)
    }
}

/// A rule's match guard. Both fields optional: omitting both means "always".
#[derive(Debug, Clone, Default, Deserialize)]
pub struct When {
    #[serde(default, rename = "type")]
    pub doc_type: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    /// Applies only to documents carrying this tag (exact tag or tag key — see
    /// [`Frontmatter::has_tag`](crate::frontmatter::Frontmatter::has_tag)).
    #[serde(default)]
    pub tag: Option<String>,
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
    /// Holds when the document carries this tag (exact tag or tag key).
    #[serde(default)]
    pub tag: Option<String>,
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

/// Built-in column keys for the TUI list (everything else is a custom field).
pub const BUILTIN_COLUMNS: [&str; 7] = [
    "id", "type", "title", "status", "tags", "created", "updated",
];

fn default_columns() -> Vec<String> {
    ["id", "title", "status", "tags"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// `[tui]` — presentation knobs for the `opys tui` board (ignored by the core
/// engine, validated here so `config validate` catches mistakes).
#[derive(Debug, Clone, Deserialize)]
pub struct TuiConfig {
    /// The list columns, left to right. Each is a [`BUILTIN_COLUMNS`] key or the
    /// name of a custom frontmatter field (shown as that field's value).
    #[serde(default = "default_columns")]
    pub columns: Vec<String>,
}

impl Default for TuiConfig {
    fn default() -> Self {
        TuiConfig {
            columns: default_columns(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectConfig {
    /// Inventory base directory, relative to the project root (the dir holding
    /// `opys.toml`). Defaults to `opys`.
    #[serde(default = "default_base")]
    pub base: String,
    #[serde(default = "default_pad")]
    pub pad: usize,
    #[serde(default)]
    pub layout: Layout,
    #[serde(default)]
    pub types: BTreeMap<String, DocType>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Presentation rules for the TUI. Ignored by the core engine; parsed and
    /// validated here so `config validate` catches mistakes regardless of the
    /// `tui` feature. See [`crate::palette`].
    #[serde(default)]
    pub palette: BTreeMap<String, PaletteEntry>,
    /// TUI list presentation. Ignored by the core engine; see [`TuiConfig`].
    #[serde(default)]
    pub tui: TuiConfig,
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

    /// The canonical file path for a document, relative to the inventory base:
    /// the `layout.path` template with `{type}`/`{status}`/`{id}` substituted and
    /// empty path segments collapsed. The `{type}`/`{status}` segments come from
    /// the type matching `id`'s prefix (empty for an unknown prefix).
    pub fn doc_relpath(&self, id: &str, status: &str) -> PathBuf {
        let t = self.type_name_for_id(id).and_then(|n| self.types.get(n));
        let type_seg = t.map_or(DEFAULT_DOC_DIR, DocType::resolved_dir);
        let status_seg = t.map_or("", |t| t.status_dir(status));
        let rendered = self
            .layout
            .path
            .replace("{type}", type_seg)
            .replace("{status}", status_seg)
            .replace("{id}", id);
        // Collapse empty segments so unset `{type}`/`{status}` don't leave `//`.
        rendered.split('/').filter(|seg| !seg.is_empty()).collect()
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

        // The layout template must place each document at a unique path: it must
        // contain `{id}` and use only the known placeholders.
        if !self.layout.path.contains("{id}") {
            errs.push("layout.path must contain the {id} placeholder".into());
        }
        for unknown in unknown_placeholders(&self.layout.path) {
            errs.push(format!(
                "layout.path: unknown placeholder '{{{unknown}}}' (use type/status/id)"
            ));
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
            for s in t.status_dirs.keys() {
                if !t.statuses.contains(s) {
                    errs.push(format!(
                        "type '{name}': status_dirs key '{s}' is not a status"
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
                for (ci, chk) in sec.checks.iter().enumerate() {
                    validate_check(name, &sec.heading, sec.kind, ci + 1, chk, &mut errs);
                }
                validate_parts(name, sec, &mut errs);
            }
        }

        for (i, rule) in self.rules.iter().enumerate() {
            self.validate_rule(i + 1, rule, &type_names, &mut errs);
        }

        self.validate_palette(&type_names, &mut errs);
        self.validate_tui(&mut errs);
        errs
    }

    /// Validate `[tui].columns`: each must be a built-in key or a custom field
    /// declared on some type.
    fn validate_tui(&self, errs: &mut Vec<String>) {
        let known_field = |key: &str| self.types.values().any(|t| t.fields.contains_key(key));
        for col in &self.tui.columns {
            if !BUILTIN_COLUMNS.contains(&col.as_str()) && !known_field(col) {
                errs.push(format!(
                    "tui.columns: '{col}' is not a built-in column or a declared field"
                ));
            }
        }
    }

    /// Validate the `[palette]` rules: every matcher must reference a real type
    /// and/or a real status, the colors must parse, and each entry needs ≥1
    /// matcher.
    fn validate_palette(&self, types: &HashSet<&str>, errs: &mut Vec<String>) {
        for (name, entry) in &self.palette {
            if entry.matchers.is_empty() {
                errs.push(format!("palette '{name}': needs at least one matcher"));
            }
            for m in &entry.matchers {
                if let Some(t) = &m.doc_type {
                    if !types.contains(t.as_str()) {
                        errs.push(format!(
                            "palette '{name}': matcher type '{t}' is not a defined type"
                        ));
                    }
                }
                if let Some(s) = &m.status {
                    let ok = match &m.doc_type {
                        // When the matcher also fixes a type, the status must be
                        // one of that type's statuses; otherwise it must be a
                        // status of some type.
                        Some(t) => self.types.get(t).is_some_and(|dt| dt.statuses.contains(s)),
                        None => self.types.values().any(|dt| dt.statuses.contains(s)),
                    };
                    if !ok {
                        let scope = m
                            .doc_type
                            .as_ref()
                            .map(|t| format!(" of type '{t}'"))
                            .unwrap_or_default();
                        errs.push(format!(
                            "palette '{name}': matcher status '{s}' is not a status{scope}"
                        ));
                    }
                }
            }
            for (label, color) in [
                ("fg_color", &entry.style.fg_color),
                ("bg_color", &entry.style.bg_color),
            ] {
                if let Some(c) = color {
                    if parse_color(c).is_none() {
                        errs.push(format!(
                            "palette '{name}': {label} '{c}' is not a valid color (a name, #rrggbb, or 0-255)"
                        ));
                    }
                }
            }
        }
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
                    term.tag.is_some(),
                ]
                .iter()
                .filter(|b| **b)
                .count();
                if count != 1 {
                    errs.push(format!(
                        "{tag}: each require_any term needs exactly one of field/link/section/tag"
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

/// Validate a section's `[[parts]]`: only a `structured` section may declare
/// them, and then it must declare at least one, each with a non-empty, unique,
/// colon-free label.
fn validate_parts(type_name: &str, sec: &SectionSpec, errs: &mut Vec<String>) {
    let tag = format!("type '{type_name}' section '{}'", sec.heading);
    if sec.kind != SectionKind::Structured {
        if !sec.parts.is_empty() {
            errs.push(format!(
                "{tag}: 'parts' is only allowed on a 'structured' section"
            ));
        }
        return;
    }
    if sec.parts.is_empty() {
        errs.push(format!(
            "{tag}: a 'structured' section needs at least one part"
        ));
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for p in &sec.parts {
        if p.label.trim().is_empty() {
            errs.push(format!("{tag}: a part label must not be empty"));
        } else if p.label.contains(':') {
            errs.push(format!(
                "{tag}: part label '{}' must not contain ':'",
                p.label
            ));
        }
        if !seen.insert(p.label.as_str()) {
            errs.push(format!("{tag}: duplicate part label '{}'", p.label));
        }
    }
}

/// Validate one [`SectionCheck`]: its `pattern` compiles, `file` / every
/// `${group}` reference name real capture groups, at least one of `file` /
/// `must_match` is set, `must_match` compiles, and `scope = "checked"` only
/// targets a checklist section.
fn validate_check(
    type_name: &str,
    heading: &str,
    kind: SectionKind,
    n: usize,
    chk: &SectionCheck,
    errs: &mut Vec<String>,
) {
    let tag = format!("type '{type_name}' section '{heading}' check #{n}");
    let names: HashSet<String> = match Regex::new(&chk.pattern) {
        Ok(re) => re
            .capture_names()
            .flatten()
            .map(|s| s.to_string())
            .collect(),
        Err(_) => {
            errs.push(format!("{tag}: pattern is not a valid regex"));
            return;
        }
    };

    if let Some(f) = &chk.file {
        if !names.contains(f) {
            errs.push(format!(
                "{tag}: file '{f}' is not a named capture group of pattern"
            ));
        }
    }
    if chk.file.is_none() && chk.must_match.is_none() {
        errs.push(format!("{tag}: needs at least one of file / must_match"));
    }

    let group_re = Regex::new(r"\$\{(\w+)\}").unwrap();
    for tmpl in [chk.must_match.as_deref(), chk.message.as_deref()]
        .into_iter()
        .flatten()
    {
        for c in group_re.captures_iter(tmpl) {
            let g = &c[1];
            if !names.contains(g) {
                errs.push(format!(
                    "{tag}: ${{{g}}} is not a named capture group of pattern"
                ));
            }
        }
    }
    if let Some(mm) = &chk.must_match {
        let probe = group_re.replace_all(mm, "x");
        if Regex::new(&probe).is_err() {
            errs.push(format!("{tag}: must_match is not a valid regex"));
        }
    }
    if chk.scope == CheckScope::Checked && !kind.is_checkable() {
        errs.push(format!(
            "{tag}: scope = \"checked\" requires a checklist section"
        ));
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
    fn flags_structured_parts_problems() {
        // A structured section with no parts, and parts on a non-structured
        // section, are both rejected; a bad label is flagged too.
        let cfg: ProjectConfig = toml::from_str(
            r#"
[types.feature]
prefix = "FEAT"
statuses = ["planned"]
default_status = "planned"

[[types.feature.sections]]
heading = "Empty"
kind = "structured"

[[types.feature.sections]]
heading = "Steps"
kind = "structured"
[[types.feature.sections.parts]]
label = "Bad:Label"

[[types.feature.sections]]
heading = "Notes"
kind = "prose"
[[types.feature.sections.parts]]
label = "Whatever"
"#,
        )
        .unwrap();
        let joined = cfg.validate().join("\n");
        assert!(
            joined.contains("section 'Empty': a 'structured' section needs at least one part"),
            "{joined}"
        );
        assert!(
            joined.contains("section 'Notes': 'parts' is only allowed on a 'structured' section"),
            "{joined}"
        );
        assert!(
            joined.contains("section 'Steps': part label 'Bad:Label' must not contain ':'"),
            "{joined}"
        );
    }

    #[test]
    fn flags_unknown_tui_column() {
        let cfg: ProjectConfig = toml::from_str(
            r#"
[types.feature]
prefix = "FEAT"
statuses = ["planned"]
default_status = "planned"
[types.feature.fields.priority]
type = "string"

[tui]
columns = ["id", "title", "priority", "bogus"]
"#,
        )
        .unwrap();
        let joined = cfg.validate().join("\n");
        assert!(joined.contains("tui.columns: 'bogus'"), "{joined}");
        // built-ins and declared fields are accepted
        assert!(!joined.contains("'priority'"), "{joined}");
        assert!(!joined.contains("'id'"), "{joined}");
    }

    #[test]
    fn flags_palette_unknown_type_status_and_bad_color() {
        let cfg: ProjectConfig = toml::from_str(
            r#"
[types.feature]
prefix = "FEAT"
statuses = ["planned", "done"]
default_status = "planned"

[palette.ghost]
matchers = [ { type = "ghost" } ]

[palette.badstatus]
matchers = [ { status = "nope" } ]

[palette.typedstatus]
matchers = [ { type = "feature", status = "nope" } ]

[palette.badcolor]
matchers = [ { status = "done" } ]
[palette.badcolor.style]
fg_color = "rainbow"

[palette.empty]
matchers = []
"#,
        )
        .unwrap();
        let joined = cfg.validate().join("\n");
        assert!(
            joined.contains("matcher type 'ghost' is not a defined type"),
            "{joined}"
        );
        assert!(
            joined.contains("matcher status 'nope' is not a status\n")
                || joined.contains("matcher status 'nope' is not a status of"),
            "{joined}"
        );
        assert!(
            joined.contains("matcher status 'nope' is not a status of type 'feature'"),
            "{joined}"
        );
        assert!(
            joined.contains("fg_color 'rainbow' is not a valid color"),
            "{joined}"
        );
        assert!(
            joined.contains("palette 'empty': needs at least one matcher"),
            "{joined}"
        );
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
