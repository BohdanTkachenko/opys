//! JSON Schema generation for editor / CI validation.
//!
//! Two schemas: one for `_config.toml` (static), and one for feature
//! frontmatter that is *derived from a project's config* so declared custom
//! fields are recognized and undeclared keys are rejected — the point being to
//! stop an agent from hallucinating fields.

use serde_json::{json, Map, Value};

use crate::config::{Config, FieldType, CORE_STATUSES};

/// JSON Schema for `_config.toml`.
pub fn config_schema() -> Value {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "title": "opys project config",
        "type": "object",
        "properties": {
            "prefix": { "type": "string", "description": "Feature ID prefix, e.g. VIK -> VIK-0001" },
            "pad": { "type": "integer", "minimum": 1, "description": "Zero-padding width" },
            "test_search_paths": { "type": "array", "items": { "type": "string" } },
            "test_reference_check": { "enum": ["grep", "extract", "none"] },
            "test_name_pattern": { "type": "string", "description": "Regex with one capture group extracting test names" },
            "extra_statuses": { "type": "array", "items": { "type": "string" } },
            "parity": { "type": "boolean" },
            "fields": {
                "type": "object",
                "additionalProperties": {
                    "type": "object",
                    "properties": {
                        "type": { "enum": ["string", "list", "bool", "int"] },
                        "required": { "type": "boolean" },
                        "description": { "type": "string" }
                    },
                    "additionalProperties": false
                }
            }
        },
        "additionalProperties": false
    })
}

/// JSON Schema for feature frontmatter, derived from `cfg`.
pub fn frontmatter_schema(cfg: &Config) -> Value {
    let mut props = Map::new();

    let id_pattern = format!("^{}-[0-9]{{{},}}$", regex_escape(&cfg.prefix), cfg.pad);
    props.insert(
        "id".into(),
        json!({ "type": "string", "pattern": id_pattern }),
    );

    let mut statuses: Vec<String> = CORE_STATUSES.iter().map(|s| s.to_string()).collect();
    statuses.extend(cfg.extra_statuses.iter().cloned());
    props.insert("status".into(), json!({ "enum": statuses }));

    props.insert(
        "tags".into(),
        json!({
            "type": "array",
            "minItems": 1,
            "items": { "type": "string", "pattern": "^[a-z0-9]+(-[a-z0-9]+)*$" }
        }),
    );
    props.insert("wontfix_reason".into(), json!({ "type": "string" }));
    props.insert("spec".into(), json!({ "type": "string" }));

    let mut required = vec!["id".to_string(), "status".to_string(), "tags".to_string()];
    for (name, spec) in &cfg.fields {
        let mut node = Map::new();
        node.insert(
            "type".into(),
            Value::String(json_type(spec.field_type).into()),
        );
        if let Some(desc) = &spec.description {
            node.insert("description".into(), Value::String(desc.clone()));
        }
        props.insert(name.clone(), Value::Object(node));
        if spec.required {
            required.push(name.clone());
        }
    }

    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "title": "opys feature frontmatter",
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": false
    })
}

fn json_type(t: FieldType) -> &'static str {
    match t {
        FieldType::String => "string",
        FieldType::List => "array",
        FieldType::Bool => "boolean",
        FieldType::Int => "integer",
    }
}

fn regex_escape(s: &str) -> String {
    regex::escape(s)
}
