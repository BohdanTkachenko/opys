//! Project configuration loaded from `<inventory>/_config.toml`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::{OpysError, Result};

/// The four statuses every project has, in lifecycle order.
pub const CORE_STATUSES: [&str; 4] = ["planned", "partial", "implemented", "wontfix"];

/// Fixed ID prefix for features. Not configurable — a feature ID is always
/// `FEAT-NNNN` so cross-project references are unambiguous.
pub const FEAT_PREFIX: &str = "FEAT";

/// Work-item statuses, in lifecycle order. `done` is terminal and only reached
/// via `work-item close` (which deletes or archives the file).
pub const WI_CORE_STATUSES: [&str; 4] = ["todo", "in-progress", "blocked", "done"];

/// A hardcoded kind of work item. All work-item types share the one ephemeral
/// lifecycle (link a feature, `## Tasks`/`## Progress`, close-deletes); they
/// differ only by ID prefix and a few per-type required sections. The type of a
/// work item is *derived from its ID prefix* — there is no `type:` frontmatter
/// field, keeping the ID the single source of truth.
#[derive(Debug, Clone, Copy)]
pub struct WorkItemType {
    pub name: &'static str,
    pub prefix: &'static str,
    /// Required body sections beyond the shared `required_sections` baseline.
    pub extra_required_sections: &'static [&'static str],
}

impl WorkItemType {
    /// Effective required sections for this type: the configured `baseline`
    /// first, then any per-type extras not already present (order-preserving).
    pub fn required_sections(&self, baseline: &[String]) -> Vec<String> {
        let mut out = baseline.to_vec();
        for s in self.extra_required_sections {
            if !out.iter().any(|b| b == s) {
                out.push(s.to_string());
            }
        }
        out
    }
}

/// The fixed set of work-item types. The first entry is the default for
/// `work-item new` when `--type` is omitted.
pub const WORK_ITEM_TYPES: [WorkItemType; 3] = [
    WorkItemType {
        name: "task",
        prefix: "TASK",
        extra_required_sections: &[],
    },
    WorkItemType {
        name: "bug",
        prefix: "BUG",
        extra_required_sections: &["Reproduction"],
    },
    WorkItemType {
        name: "chore",
        prefix: "CHORE",
        extra_required_sections: &[],
    },
];

/// The work-item type whose ID prefix matches `id`, if any.
pub fn type_for_id(id: &str) -> Option<&'static WorkItemType> {
    let prefix = id.split_once('-').map(|(p, _)| p)?;
    WORK_ITEM_TYPES.iter().find(|t| t.prefix == prefix)
}

/// The work-item type with the given `name` (e.g. `"bug"`), if any.
pub fn type_by_name(name: &str) -> Option<&'static WorkItemType> {
    WORK_ITEM_TYPES.iter().find(|t| t.name == name)
}

/// Whether `id` carries a known work-item prefix (`BUG-`/`TASK-`/`CHORE-`).
pub fn is_work_item_id(id: &str) -> bool {
    type_for_id(id).is_some()
}

/// Every configured work-item ID prefix.
pub fn work_item_prefixes() -> impl Iterator<Item = &'static str> {
    WORK_ITEM_TYPES.iter().map(|t| t.prefix)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TestRefCheck {
    /// Every test reference must appear as a substring under `test_search_paths`.
    #[default]
    Grep,
    /// Extract real test names with `test_name_pattern` and resolve each
    /// reference against that set (and, for `path::name` refs, the named file).
    Extract,
    /// Skip test-reference existence checking.
    None,
}

/// Declared type of a custom frontmatter field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    #[default]
    String,
    List,
    Bool,
    Int,
    /// A string constrained to the declared `values` set (see [`FieldSpec`]).
    Enum,
}

impl FieldType {
    pub fn as_str(self) -> &'static str {
        match self {
            FieldType::String => "string",
            FieldType::List => "list",
            FieldType::Bool => "bool",
            FieldType::Int => "int",
            FieldType::Enum => "enum",
        }
    }
}

/// Declaration of a project-specific frontmatter field.
#[derive(Debug, Clone, Deserialize)]
pub struct FieldSpec {
    #[serde(default, rename = "type")]
    pub field_type: FieldType,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub description: Option<String>,
    /// Allowed values for an `enum` field; ignored for other types.
    #[serde(default)]
    pub values: Vec<String>,
    /// Optional regex a `string` value must fully match (universal-config only).
    #[serde(default)]
    pub pattern: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_pad")]
    pub pad: usize,
    #[serde(default = "default_search_paths")]
    pub test_search_paths: Vec<String>,
    #[serde(default)]
    pub test_reference_check: TestRefCheck,
    /// Regex with one capture group extracting test names from source files.
    /// Required when `test_reference_check = "extract"`.
    #[serde(default)]
    pub test_name_pattern: Option<String>,
    #[serde(default)]
    pub extra_statuses: Vec<String>,
    /// Report feature-parity percentages (only meaningful for parity projects).
    #[serde(default)]
    pub parity: bool,
    #[serde(default)]
    pub fields: BTreeMap<String, FieldSpec>,
}

fn default_pad() -> usize {
    4
}
fn default_search_paths() -> Vec<String> {
    vec!["src".to_string(), "tests".to_string()]
}
fn default_required_sections() -> Vec<String> {
    vec!["Tasks".to_string(), "Progress".to_string()]
}

/// Configuration for the optional work-item subsystem, loaded from
/// `<base>/work-items/_config.toml`. The subsystem is enabled only when that
/// file exists; otherwise the project is features-only.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkItemConfig {
    #[serde(default = "default_pad")]
    pub pad: usize,
    /// Statuses beyond the core todo | in-progress | blocked | done.
    #[serde(default)]
    pub extra_statuses: Vec<String>,
    /// Body sections that must be present (verified, and scaffolded by `new`).
    #[serde(default = "default_required_sections")]
    pub required_sections: Vec<String>,
    #[serde(default)]
    pub fields: BTreeMap<String, FieldSpec>,
}

impl WorkItemConfig {
    /// Load the work-item config if present; `None` means the subsystem is not
    /// configured for this project.
    pub fn load_optional(path: &Path) -> Result<Option<WorkItemConfig>> {
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(path)?;
        toml::from_str(&text)
            .map(Some)
            .map_err(|source| OpysError::Toml {
                path: path.to_path_buf(),
                source,
            })
    }

    /// Core work-item statuses plus any project-configured extras.
    pub fn statuses(&self) -> Vec<String> {
        let mut out: Vec<String> = WI_CORE_STATUSES.iter().map(|s| s.to_string()).collect();
        out.extend(self.extra_statuses.iter().cloned());
        out
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        if !path.exists() {
            return Err(OpysError::ConfigNotFound(path.to_path_buf()));
        }
        let text = std::fs::read_to_string(path)?;
        toml::from_str(&text).map_err(|source| OpysError::Toml {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Core statuses plus any project-configured extras.
    pub fn statuses(&self) -> Vec<String> {
        let mut out: Vec<String> = CORE_STATUSES.iter().map(|s| s.to_string()).collect();
        out.extend(self.extra_statuses.iter().cloned());
        out
    }

    pub fn grep_mode(&self) -> bool {
        self.test_reference_check == TestRefCheck::Grep
    }

    pub fn extract_mode(&self) -> bool {
        self.test_reference_check == TestRefCheck::Extract
    }
}
