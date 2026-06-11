//! Project configuration loaded from `<inventory>/_config.toml`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::{OpysError, Result};

/// The four statuses every project has, in lifecycle order.
pub const CORE_STATUSES: [&str; 4] = ["planned", "partial", "implemented", "wontfix"];

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
}

impl FieldType {
    pub fn as_str(self) -> &'static str {
        match self {
            FieldType::String => "string",
            FieldType::List => "list",
            FieldType::Bool => "bool",
            FieldType::Int => "int",
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
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_prefix")]
    pub prefix: String,
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

fn default_prefix() -> String {
    "FEAT".to_string()
}
fn default_pad() -> usize {
    4
}
fn default_search_paths() -> Vec<String> {
    vec!["src".to_string(), "tests".to_string()]
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
