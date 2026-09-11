//! `opys list` — slice the inventory with one SQL SELECT over the corpus
//! store: a JOIN per `--tag`/`--field` filter (AND semantics), column
//! predicates for `--type`/`--status`, DISTINCT against multi-row matches,
//! ordered by load (path) order.
//!
//! `--blocked`/`--unblocked` is the one filter that is *not* SQL. Blocked is
//! derived (ADR-0099 decision 3) from three things no single table holds —
//! whether an entry is struck, whether its target still exists, and whether
//! that target's type releases at its status — and expressing it would need
//! exactly the correlated subqueries the store forbids. It runs in Rust over
//! the reconstructed corpus, and only when one of the two flags is passed. All
//! it needs is [`refs::Corpus`] — an id → status index — so that is all it
//! builds.
//!
//! `--sort` is Rust for the same reason and then some (ADR-0099 decision 4):
//! effective priority is a walk of the transitive `blocks` closure, which is a
//! recursive query the store has no answer for. It is computed over the *whole*
//! corpus, never the filtered rows — a chain of blocked work can run through a
//! document `--type`/`--status`/`--tag` left out, and the number it contributes
//! is no less real for that. Without the flag neither the closure nor a line of
//! the sort runs, and the listing is byte-identical to load order.

use std::collections::{BTreeMap, HashMap, HashSet};

use gluesql::prelude::{ParamLiteral, Value as GValue};

use crate::cli::ListFormat;
use crate::commands::{parse_field_filters, print_lines};
use crate::doc::Doc;
use crate::error::Result;
use crate::order;
use crate::project_config::ProjectConfig;
use crate::refs;
use crate::store::{g_i64, g_str, IntoParam};
use crate::Ctx;

/// Which group a document sorts into under `--sort`: the position of its type
/// among the configured ones, in the order `opys.toml` declares them
/// ([`crate::project_config::TypeMap`] keeps that order through parsing).
/// Documents whose id prefix matches no type form a final group, ordered like
/// any other by the default chain.
///
/// Declaration order rather than type-*name* order: the config is the only
/// place a project says which of its types it leads with, and alphabetising
/// them would impose an order it never asked for.
fn group_of(doc: &Doc, pcfg: &ProjectConfig) -> usize {
    doc.id()
        .and_then(|id| pcfg.type_name_for_id(id))
        .and_then(|name| pcfg.types.index_of(name))
        .unwrap_or(usize::MAX)
}

// One more parameter than clippy's ceiling, as `new::core` already is: the
// alternative is an options struct whose every field is one flag of one
// command, which reads worse at both ends.
#[allow(clippy::too_many_arguments)]
pub fn run(
    ctx: &Ctx,
    type_name: Option<&str>,
    tag: Option<&str>,
    status: Option<&str>,
    field: &[String],
    blocked: Option<bool>,
    sort: bool,
    format: ListFormat,
) -> Result<()> {
    let prj = ctx.open()?;
    let filters = parse_field_filters(field)?;
    let (mut store, _) = ctx.load(&prj)?;

    // Ahead of the SELECT: both want `&mut store`. Each flag pays only for what
    // it reads — `--blocked` for the id → status index, `--sort` for the
    // priority closure on top of it — and when both are passed the sort's
    // richer index serves the filter too, so nothing is built twice. A plain
    // `list` reconstructs nothing and costs exactly what it did.
    let mut blocked_ids: Option<HashSet<String>> = None;
    let mut sort_rank: Option<HashMap<String, usize>> = None;
    if blocked.is_some() || sort {
        let docs: Vec<Doc> = store.all_docs()?.into_iter().map(|(_, d)| d).collect();
        let ordered = sort.then(|| order::Corpus::of_docs(&prj.pcfg, &docs));
        if blocked.is_some() {
            // `--blocked` alone needs the id → status index and nothing else:
            // no closure walk, which is the whole cost of `order::Corpus`. With
            // `--sort` the index that walk already built serves this too.
            let plain;
            let blockers = match &ordered {
                Some(corpus) => corpus.blockers(),
                None => {
                    plain = refs::Corpus::of_docs(&prj.pcfg, &docs);
                    &plain
                }
            };
            blocked_ids = Some(
                docs.iter()
                    .filter(|d| refs::is_blocked(d, blockers))
                    .filter_map(|d| d.id().map(str::to_string))
                    .collect(),
            );
        }
        if let Some(corpus) = &ordered {
            // Group and chain once per document, never inside the comparator:
            // `chain_for` allocates, and a sort asks O(n log n) times.
            let mut ranked: Vec<(usize, &Doc)> =
                docs.iter().map(|d| (group_of(d, &prj.pcfg), d)).collect();
            let mut chains: HashMap<usize, Vec<order::OrderKey>> = HashMap::new();
            for (group, doc) in &ranked {
                // Same group means same type, so either document's chain is the
                // group's chain.
                chains
                    .entry(*group)
                    .or_insert_with(|| corpus.chain_for(doc));
            }
            // Stable, so two documents no chain separates keep load order.
            ranked.sort_by(|(ga, a), (gb, b)| {
                ga.cmp(gb)
                    .then_with(|| order::compare(a, b, &chains[ga], corpus))
            });
            sort_rank = Some(
                ranked
                    .iter()
                    .enumerate()
                    .filter_map(|(i, (_, d))| d.id().map(|id| (id.to_string(), i)))
                    .collect(),
            );
        }
    }

    // Build the one SELECT: joins for tag/field filters, predicates for
    // type/status. Values always as parameters.
    let mut sql =
        String::from("SELECT DISTINCT d.dkey, d.id, d.status, d.title, d.path FROM docs d");
    let mut wheres: Vec<String> = Vec::new();
    let mut params: Vec<ParamLiteral> = Vec::new();
    let mut n = 0usize;
    let mut next = |params: &mut Vec<ParamLiteral>, v: String| {
        params.push(v.into_param());
        n += 1;
        format!("${n}")
    };

    if let Some(q) = tag {
        // Matches an exact tag or any tag whose key equals the query.
        let p = next(&mut params, q.to_string());
        sql.push_str(&format!(
            " JOIN tags tq ON tq.dkey = d.dkey AND (tq.tag = {p} OR tq.key = {p})"
        ));
    }
    if let Some(t) = type_name {
        let p = next(&mut params, t.to_string());
        wheres.push(format!("d.type = {p}"));
    }
    if let Some(s) = status {
        let p = next(&mut params, s.to_string());
        wheres.push(format!("d.status = {p}"));
    }
    for (i, (key, want)) in filters.iter().enumerate() {
        // `--field key=value` matches wherever the key lives: a core column
        // (right-typed), the tags list (element equality), or an fm_fields
        // fidelity/element row (scalar_str coercion) — same semantics as the
        // old in-memory `field_matches`.
        let pk = next(&mut params, key.clone());
        let pv = next(&mut params, want.clone());
        sql.push_str(&format!(
            " LEFT JOIN fm_fields f{i} ON f{i}.dkey = d.dkey AND f{i}.key = {pk} AND f{i}.value = {pv}"
        ));
        let mut arms = vec![format!("f{i}.value IS NOT NULL")];
        match key.as_str() {
            "id" | "status" | "created" | "updated" => {
                arms.push(format!("d.{key} = {pv}"));
            }
            "tags" => {
                sql.push_str(&format!(
                    " LEFT JOIN tags tf{i} ON tf{i}.dkey = d.dkey AND tf{i}.tag = {pv}"
                ));
                arms.push(format!("tf{i}.tag IS NOT NULL"));
            }
            _ => {}
        }
        wheres.push(format!("({})", arms.join(" OR ")));
    }
    if !wheres.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&wheres.join(" AND "));
    }
    sql.push_str(" ORDER BY d.dkey");

    let (_, rows) = store.select(&sql, params)?;

    // Table format needs each doc's tags, joined in list order.
    let doc_tags: BTreeMap<i64, Vec<String>> = if matches!(format, ListFormat::Table) {
        let (_, trows) = store.select("SELECT dkey, tag FROM tags ORDER BY dkey, seq", vec![])?;
        let mut m: BTreeMap<i64, Vec<String>> = BTreeMap::new();
        for r in trows {
            if let (Some(k), Some(t)) = (g_i64(&r[0]), g_str(&r[1])) {
                m.entry(k).or_default().push(t);
            }
        }
        m
    } else {
        BTreeMap::new()
    };

    let mut kept: Vec<&Vec<GValue>> = rows
        .iter()
        .filter(|r| match (&blocked_ids, blocked) {
            (Some(ids), Some(want)) => {
                ids.contains(g_str(&r[1]).unwrap_or_default().as_str()) == want
            }
            _ => true,
        })
        .collect();
    if let Some(rank) = &sort_rank {
        // Stable again: a row with no id (impossible today, cheap to survive)
        // keeps its place at the end rather than reordering the rest.
        kept.sort_by_key(|r| {
            rank.get(g_str(&r[1]).unwrap_or_default().as_str())
                .copied()
                .unwrap_or(usize::MAX)
        });
    }

    let lines: Vec<String> = kept
        .iter()
        .map(|r| {
            let dkey = g_i64(&r[0]).unwrap_or(0);
            let id = g_str(&r[1]).unwrap_or_default();
            let status = g_str(&r[2]).unwrap_or_default();
            let title = g_str(&r[3]).unwrap_or_default();
            let relpath = g_str(&r[4]).unwrap_or_default();
            match format {
                ListFormat::Ids => id,
                ListFormat::Paths => prj.base.join(relpath).display().to_string(),
                ListFormat::Table => {
                    let tags = doc_tags
                        .get(&dkey)
                        .map(|ts| ts.join(", "))
                        .unwrap_or_default();
                    format!("{id}  [{status}]  ({tags})  {title}")
                }
            }
        })
        .collect();
    // `list` is the producer of the documented bulk pipe, so a reader that
    // closes early (`| head -1`, `| fzf`) must not crash it.
    print_lines(lines)
}
