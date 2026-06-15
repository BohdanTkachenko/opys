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

/// Fixed ID prefix for work items: `WI-NNNN`.
pub const WI_PREFIX: &str = "WI";

/// Work-item statuses, in lifecycle order. `done` is terminal and only reached
/// via `work-item close` (which deletes or archives the file).
pub const WI_CORE_STATUSES: [&str; 4] = ["todo", "in-progress", "blocked", "done"];

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
