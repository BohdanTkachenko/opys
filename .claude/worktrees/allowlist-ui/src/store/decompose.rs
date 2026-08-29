//! Doc → rows. The single classification point: every frontmatter key is
//! materialized into exactly ONE authoritative home, so reconstruction can
//! never lose or duplicate data.
//!
//! - `id`/`status`/`created`/`updated` → `docs` columns, **only when the value
//!   is a YAML string**; any other shape (e.g. `status: 5`) round-trips
//!   verbatim through `fm_fields` and leaves the column NULL (verify sees the
//!   wrong-typed value through the reconstructed frontmatter).
//! - `tags` → `tags` rows, only when it is a **non-empty all-string sequence**
//!   (list order preserved via `seq`); `tags: []` / wrong shapes → `fm_fields`.
//! - `references`/`blocked_by`/`blocks` → `relations` rows, only when the value
//!   is a **non-empty mapping with string keys and string values** (entry order
//!   preserved via `seq`; opys writes them sorted by id number but hand-edited
//!   order must round-trip). `raw_value` keeps any `~~strike~~`; `title`/
//!   `struck` are derived here and nowhere else.
//! - everything else (declared custom fields, composite values) → one
//!   `fm_fields` row whose `value_yaml` is the bare canonical YAML of the value
//!   (the fidelity source of truth; `value`/`kind` are query-only projections).
//!
//! Non-string top-level keys are ignored — `frontmatter::serialize` already
//! drops them, so ignoring them here is exact round-trip parity.

use serde_norway::Value as Yaml;

use crate::body;
use crate::doc::Doc;
use crate::error::{OpysError, Result};
use crate::project_config::ProjectConfig;
use crate::refs;

/// A document decomposed into table rows (not yet inserted).
pub(crate) struct DocRows {
    pub id: Option<String>,
    pub num: Option<i64>,
    pub type_name: Option<String>,
    pub status: Option<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub title: String,
    pub body: String,
    pub tags: Vec<TagRow>,
    pub relations: Vec<RelRow>,
    pub fm_fields: Vec<FmRow>,
}

pub(crate) struct TagRow {
    pub seq: i64,
    pub tag: String,
    pub key: String,
    pub value: Option<String>,
}

pub(crate) struct RelRow {
    pub field: &'static str,
    pub seq: i64,
    pub ref_id: String,
    pub ref_num: Option<i64>,
    pub raw_value: String,
    pub title: String,
    pub struck: bool,
}

pub(crate) struct FmRow {
    pub key: String,
    /// The whole value as bare canonical YAML — the fidelity source of truth.
    /// `None` marks a query-only *element* row (one per scalar element of a
    /// sequence value, so `--field key=value` can match lists in SQL);
    /// reconstruction ignores those.
    pub value_yaml: Option<String>,
    pub value: Option<String>,
    pub kind: &'static str,
}

/// The numeric part of a `PREFIX-NNNN` id if it parses (sequence-max parity
/// with `project::id_part`: malformed ids contribute NULL, which MAX ignores).
pub(crate) fn id_num(id: &str) -> Option<i64> {
    id.rsplit_once('-').and_then(|(_, n)| n.parse::<i64>().ok())
}

/// Split a tag into its key and optional value at the first `:` or `=`
/// (`area:parsing` → `("area", Some("parsing"))`; `osc` → `("osc", None)`).
pub(crate) fn split_tag(t: &str) -> (&str, Option<&str>) {
    match t.find([':', '=']) {
        Some(i) => (&t[..i], Some(&t[i + 1..])),
        None => (t, None),
    }
}

/// Decompose one document into rows.
pub(crate) fn decompose(pcfg: &ProjectConfig, doc: &Doc) -> Result<DocRows> {
    let mut out = DocRows {
        id: None,
        num: None,
        type_name: None,
        status: None,
        created: None,
        updated: None,
        title: body::title(&doc.body),
        body: doc.body.clone(),
        tags: Vec::new(),
        relations: Vec::new(),
        fm_fields: Vec::new(),
    };

    for (k, v) in &doc.frontmatter.map {
        // Non-string keys are dropped by the serializer; parity is to skip them.
        let Some(key) = k.as_str() else { continue };
        match key {
            "id" | "status" | "created" | "updated" if v.is_string() => {
                let s = v.as_str().expect("checked is_string").to_string();
                match key {
                    "id" => out.id = Some(s),
                    "status" => out.status = Some(s),
                    "created" => out.created = Some(s),
                    "updated" => out.updated = Some(s),
                    _ => unreachable!(),
                }
            }
            "tags" if is_string_seq(v) => {
                let Yaml::Sequence(seq) = v else {
                    unreachable!()
                };
                for (i, item) in seq.iter().enumerate() {
                    let tag = item.as_str().expect("checked all-string").to_string();
                    let (tkey, tval) = split_tag(&tag);
                    out.tags.push(TagRow {
                        seq: i as i64,
                        key: tkey.to_string(),
                        value: tval.map(str::to_string),
                        tag,
                    });
                }
            }
            _ if relation_field(key).is_some() && is_string_map(v) => {
                let field = relation_field(key).expect("just checked");
                let Yaml::Mapping(m) = v else { unreachable!() };
                for (i, (rk, rv)) in m.iter().enumerate() {
                    let ref_id = rk.as_str().expect("checked all-string").to_string();
                    let raw = rv.as_str().expect("checked all-string").to_string();
                    out.relations.push(RelRow {
                        field,
                        seq: i as i64,
                        ref_num: id_num(&ref_id),
                        title: refs::unstrike(&raw).to_string(),
                        struck: refs::is_struck(&raw),
                        ref_id,
                        raw_value: raw,
                    });
                }
            }
            _ => out.fm_fields.extend(fm_rows(key, v)?),
        }
    }

    out.num = out.id.as_deref().and_then(id_num);
    out.type_name = out
        .id
        .as_deref()
        .and_then(|id| pcfg.type_name_for_id(id))
        .map(str::to_string);
    Ok(out)
}

/// The `fm_fields` rows for a key: one fidelity row (bare canonical YAML +
/// scalar stringification for querying), plus one query-only element row per
/// scalar element of a sequence value (`--field key=value` list matching).
fn fm_rows(key: &str, v: &Yaml) -> Result<Vec<FmRow>> {
    let value_yaml = serde_norway::to_string(v)
        .map_err(|e| OpysError::Store(format!("cannot serialize field '{key}': {e}")))?;
    let (value, kind) = match v {
        Yaml::String(s) => (Some(s.clone()), "str"),
        Yaml::Bool(b) => (Some(b.to_string()), "bool"),
        Yaml::Number(n) if n.is_i64() || n.is_u64() => (Some(n.to_string()), "int"),
        Yaml::Number(n) => (Some(n.to_string()), "float"),
        Yaml::Null => (None, "null"),
        Yaml::Sequence(_) => (None, "seq"),
        Yaml::Mapping(_) => (None, "map"),
        Yaml::Tagged(_) => (None, "tagged"),
    };
    let mut out = vec![FmRow {
        key: key.to_string(),
        value_yaml: Some(value_yaml),
        value,
        kind,
    }];
    if let Yaml::Sequence(seq) = v {
        for item in seq {
            // scalar_str semantics: composite elements contribute no match.
            let elem = match item {
                Yaml::String(s) => Some(s.clone()),
                Yaml::Bool(b) => Some(b.to_string()),
                Yaml::Number(n) => Some(n.to_string()),
                _ => None,
            };
            if let Some(elem) = elem {
                out.push(FmRow {
                    key: key.to_string(),
                    value_yaml: None,
                    value: Some(elem),
                    kind: "elem",
                });
            }
        }
    }
    Ok(out)
}

fn relation_field(key: &str) -> Option<&'static str> {
    refs::RELATION_FIELDS.iter().find(|f| **f == key).copied()
}

/// A non-empty sequence whose every element is a string.
fn is_string_seq(v: &Yaml) -> bool {
    matches!(v, Yaml::Sequence(seq) if !seq.is_empty() && seq.iter().all(Yaml::is_string))
}

/// A non-empty mapping whose every key and value is a string.
fn is_string_map(v: &Yaml) -> bool {
    matches!(v, Yaml::Mapping(m) if !m.is_empty()
        && m.iter().all(|(k, val)| k.is_string() && val.is_string()))
}
