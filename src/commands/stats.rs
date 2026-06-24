//! `opys stats` — per-type status breakdown (counts + percentages),
//! test-plan / manual coverage, and a tag breakdown. Pure read; no
//! feature/type special-casing.

use std::collections::{BTreeMap, HashSet};

use crate::body;
use crate::doc::Doc;
use crate::error::Result;
use crate::project_config::{ProjectConfig, SectionKind};
use crate::Ctx;

/// One status row within a type: its name, count, and percent of the type total.
pub struct StatusCount {
    pub status: String,
    pub count: usize,
    pub pct: u32,
}

/// Per-type breakdown: total documents and the present statuses.
pub struct TypeStats {
    pub name: String,
    pub total: usize,
    pub by_status: Vec<StatusCount>,
}

/// One value within a keyed-tag group: the value and how many documents carry
/// a `key:value` / `key=value` tag with it.
pub struct TagValueCount {
    pub value: String,
    pub count: usize,
}

/// A keyed-tag group (`key:value` / `key=value`): the key, how many documents
/// carry any tag with this key, and the per-value breakdown (count-descending).
pub struct TagKeyStats {
    pub key: String,
    pub docs: usize,
    pub by_value: Vec<TagValueCount>,
}

/// A plain (unkeyed) tag and how many documents carry it.
pub struct TagCount {
    pub tag: String,
    pub count: usize,
}

/// Coverage for one section heading (aggregated across every document that has
/// it): how many items it holds and how many are uncovered.
pub struct SectionCoverage {
    pub heading: String,
    /// The section kind: `"checklist"`, `"structured"`, or `"log"`. For
    /// checklists `uncovered` is the count of unchecked items; for all other
    /// kinds it is always 0 (no notion of "unchecked").
    pub kind: &'static str,
    pub items: usize,
    pub uncovered: usize,
}

/// The computed stats over a set of documents — the shared data behind the CLI
/// `stats` command and the TUI stats screen (which feeds it a filtered slice).
pub struct StatsReport {
    pub total: usize,
    pub per_type: Vec<TypeStats>,
    /// Per-section coverage, keyed by the section's real heading (from config),
    /// sorted by heading then kind. Empty sections are omitted.
    pub coverage: Vec<SectionCoverage>,
    /// Keyed tags grouped by key (alphabetical), each with its value breakdown.
    pub tag_keys: Vec<TagKeyStats>,
    /// Plain tags, count-descending then alphabetical.
    pub plain_tags: Vec<TagCount>,
}

/// Split a tag into its key and optional value at the first `:` or `=`. A plain
/// tag has no value (`osc` → `("osc", None)`); a keyed tag splits at the first
/// separator (`area:parsing` → `("area", Some("parsing"))`).
fn split_tag(t: &str) -> (&str, Option<&str>) {
    match t.find([':', '=']) {
        Some(i) => (&t[..i], Some(&t[i + 1..])),
        None => (t, None),
    }
}

/// Tally tags across `docs`: keyed tags grouped by key (with per-value counts
/// and a distinct-document total per key), and plain tags counted per document.
fn tag_stats(docs: &[&Doc]) -> (Vec<TagKeyStats>, Vec<TagCount>) {
    let mut key_docs: BTreeMap<String, usize> = BTreeMap::new();
    let mut value_counts: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut plain: BTreeMap<String, usize> = BTreeMap::new();

    for d in docs {
        let tags = d.frontmatter.tags().unwrap_or_default();
        let mut keys_in_doc: HashSet<&str> = HashSet::new();
        for t in &tags {
            match split_tag(t) {
                (key, Some(value)) => {
                    *value_counts
                        .entry(key.to_string())
                        .or_default()
                        .entry(value.to_string())
                        .or_default() += 1;
                    // A key counts once per document even if it has many values.
                    if keys_in_doc.insert(key) {
                        *key_docs.entry(key.to_string()).or_default() += 1;
                    }
                }
                (tag, None) => *plain.entry(tag.to_string()).or_default() += 1,
            }
        }
    }

    let tag_keys = key_docs
        .into_iter()
        .map(|(key, docs)| {
            let mut by_value: Vec<TagValueCount> = value_counts
                .remove(&key)
                .unwrap_or_default()
                .into_iter()
                .map(|(value, count)| TagValueCount { value, count })
                .collect();
            by_value.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
            TagKeyStats {
                key,
                docs,
                by_value,
            }
        })
        .collect();

    let mut plain_tags: Vec<TagCount> = plain
        .into_iter()
        .map(|(tag, count)| TagCount { tag, count })
        .collect();
    plain_tags.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));

    (tag_keys, plain_tags)
}

/// Sum the lengths of all top-level array fields in a JSON object, giving the
/// total number of items extracted from a structured section.
fn count_array_items(data: &serde_json::Value) -> usize {
    match data {
        serde_json::Value::Object(map) => map
            .values()
            .filter_map(|v| v.as_array())
            .map(|arr| arr.len())
            .sum(),
        serde_json::Value::Array(arr) => arr.len(),
        _ => 0,
    }
}

/// Compute the stats report over `docs` (already filtered by the caller). Pure;
/// does not read the filesystem or print.
pub fn compute(pcfg: &ProjectConfig, docs: &[&Doc]) -> StatsReport {
    let mut per_type = Vec::new();
    for (tname, t) in &pcfg.types {
        let group: Vec<&&Doc> = docs
            .iter()
            .filter(|d| d.id().and_then(|id| pcfg.type_name_for_id(id)) == Some(tname.as_str()))
            .collect();
        let total = group.len();
        if total == 0 {
            continue;
        }
        let mut by_status = Vec::new();
        for status in &t.statuses {
            let count = group
                .iter()
                .filter(|d| d.status() == Some(status.as_str()))
                .count();
            if count > 0 {
                let pct = (count as f64 / total as f64 * 100.0).round() as u32;
                by_status.push(StatusCount {
                    status: status.clone(),
                    count,
                    pct,
                });
            }
        }
        per_type.push(TypeStats {
            name: tname.clone(),
            total,
            by_status,
        });
    }

    // Coverage across every type's sections, aggregated per heading+kind.
    // (heading, kind) -> (items, uncovered).
    let mut coverage: BTreeMap<(String, &'static str), (usize, usize)> = BTreeMap::new();
    for d in docs {
        let Some(tname) = d.id().and_then(|id| pcfg.type_name_for_id(id)) else {
            continue;
        };
        for sec in &pcfg.types[tname].sections {
            match sec.kind {
                SectionKind::Checklist => {
                    let items = body::checklist_items(&d.body, &sec.heading);
                    if items.is_empty() {
                        continue;
                    }
                    let unchecked = items.iter().filter(|i| !i.checked).count();
                    let e = coverage
                        .entry((sec.heading.clone(), "checklist"))
                        .or_default();
                    e.0 += items.len();
                    e.1 += unchecked;
                }
                SectionKind::Structured => {
                    if !body::has_section(&d.body, &sec.heading) {
                        continue;
                    }
                    let Some(src) = &sec.structure else { continue };
                    let Ok(schema) = crate::mdprism::parse_schema(src) else {
                        continue;
                    };
                    let content = body::section(&d.body, &sec.heading);
                    if let Ok(data) = schema.extract(&content) {
                        let count = count_array_items(&data);
                        if count > 0 {
                            let e = coverage
                                .entry((sec.heading.clone(), "structured"))
                                .or_default();
                            e.0 += count;
                        }
                    }
                }
                SectionKind::Log => {
                    let content = body::section(&d.body, &sec.heading);
                    let count = content.lines().filter(|l| l.starts_with("- ")).count();
                    if count > 0 {
                        let e = coverage.entry((sec.heading.clone(), "log")).or_default();
                        e.0 += count;
                    }
                }
                SectionKind::Prose => {} // no countable structure
            }
        }
    }
    let coverage = coverage
        .into_iter()
        .filter(|(_, (items, _))| *items > 0)
        .map(|((heading, kind), (items, uncovered))| SectionCoverage {
            heading,
            kind,
            items,
            uncovered,
        })
        .collect();

    let (tag_keys, plain_tags) = tag_stats(docs);

    StatsReport {
        total: docs.len(),
        per_type,
        coverage,
        tag_keys,
        plain_tags,
    }
}

pub fn run(ctx: &Ctx) -> Result<()> {
    let prj = ctx.open()?;
    let (docs, _) = prj.load_docs();
    let refs: Vec<&Doc> = docs.iter().collect();
    let r = compute(&prj.pcfg, &refs);

    println!("documents: {}", r.total);
    for ts in &r.per_type {
        println!("\n{}: {}", ts.name, ts.total);
        for sc in &ts.by_status {
            println!("  {:<16} {:>4}  {:>3}%", sc.status, sc.count, sc.pct);
        }
    }
    if !r.tag_keys.is_empty() {
        println!("\ntags by key:");
        for tk in &r.tag_keys {
            println!("  {} ({} docs)", tk.key, tk.docs);
            for v in &tk.by_value {
                println!("    {:<16} {:>4}", v.value, v.count);
            }
        }
    }
    if !r.plain_tags.is_empty() {
        println!("\ntags:");
        for tc in &r.plain_tags {
            println!("  {:<16} {:>4}", tc.tag, tc.count);
        }
    }

    if !r.coverage.is_empty() {
        println!("\ncoverage:");
        for c in &r.coverage {
            if c.kind == "checklist" {
                println!(
                    "  {:<16} {:<10} {} uncovered / {} items",
                    c.heading, c.kind, c.uncovered, c.items
                );
            } else {
                println!("  {:<16} {:<10} {} items", c.heading, c.kind, c.items);
            }
        }
    }
    Ok(())
}
