//! Query: run a jq filter over the data extracted from a markdown document.

use super::error::{QueryError, ValidationErrors};
use super::schema::Schema;
use jaq_core::data::JustLut;
use jaq_core::load::{Arena, File, Loader};
use jaq_core::{Ctx, Vars};
use jaq_json::{read, Val};
use serde_json::Value;

impl Schema {
    /// Extract the document into a data object, then apply the jq `filter` and
    /// return all output values.
    ///
    /// Parse/compile errors in the filter are [`QueryError::Filter`]; a
    /// non-conforming document is [`QueryError::Validation`]. Individual runtime
    /// errors (e.g. `null | .foo`) are silently dropped — matching standard jq
    /// behaviour where erroring paths simply produce no output.
    pub fn query(&self, md: &str, filter: &str) -> Result<Vec<Value>, QueryError> {
        let data = self
            .extract(md)
            .map_err(|problems| QueryError::Validation(ValidationErrors(problems)))?;

        run_jq(filter, data)
    }
}

fn run_jq(filter_str: &str, data: Value) -> Result<Vec<Value>, QueryError> {
    // serde_json::Value → JSON bytes → jaq_json::Val (avoids the serde feature dep)
    let json_bytes = serde_json::to_vec(&data)?;
    let input = read::parse_single(&json_bytes)
        .map_err(|e| QueryError::Filter(format!("value serialization: {e}")))?;

    let program = File {
        code: filter_str,
        path: (),
    };
    let defs = jaq_core::defs()
        .chain(jaq_std::defs())
        .chain(jaq_json::defs());
    let loader = Loader::new(defs);
    let arena = Arena::default();

    let modules = loader
        .load(&arena, program)
        .map_err(|errs| QueryError::Filter(format!("parse error: {errs:?}")))?;

    let funs = jaq_core::funs::<JustLut<Val>>()
        .chain(jaq_std::funs::<JustLut<Val>>())
        .chain(jaq_json::funs());
    let filter = jaq_core::Compiler::<_, JustLut<Val>>::default()
        .with_funs(funs)
        .compile(modules)
        .map_err(|errs| QueryError::Filter(format!("compile error: {errs:?}")))?;

    let ctx = Ctx::<JustLut<Val>>::new(&filter.lut, Vars::new([]));
    let results: Vec<Value> = filter
        .id
        .run((ctx, input))
        .filter_map(|r| r.ok())
        .map(val_to_json)
        .collect();

    Ok(results)
}

/// Convert a `jaq_json::Val` to `serde_json::Value` via JSON string round-trip.
fn val_to_json(v: Val) -> Value {
    serde_json::from_str(&v.to_string()).unwrap_or(Value::Null)
}
