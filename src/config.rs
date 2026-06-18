//! Shared field-spec types used by the universal config (`opys.toml`). See
//! [`crate::project_config`] for the engine config.

use serde::Deserialize;

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

/// Declaration of a project-specific frontmatter field (a type's `[fields.*]`).
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
    /// Optional regex a `string` value must fully match.
    #[serde(default)]
    pub pattern: Option<String>,
}
