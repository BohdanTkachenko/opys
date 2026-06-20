//! `opys stats` — per-type status breakdown (counts + percentages),
//! test-plan / manual coverage, and a tag breakdown. Pure read; no
//! feature/type special-casing.

use std::collections::{BTreeMap, HashSet};

use crate::body;
use crate::doc::Doc;
use crate::error::Result;
use crate::project_config::{CheckScope, ProjectConfig, SectionKind};
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

/// The computed stats over a set of documents — the shared data behind the CLI
/// `stats` command and the TUI stats screen (which feeds it a filtered slice).
pub struct StatsReport {
    pub total: usize,
    pub per_type: Vec<TypeStats>,
    pub uncovered_testplan: usize,
    pub manual_total: usize,
    pub manual_uncovered: usize,
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

    // Test-plan / manual coverage across every type's relevant sections.
    let mut uncovered_testplan = 0usize;
    let mut manual_total = 0usize;
    let mut manual_uncovered = 0usize;
    for d in docs {
        let Some(tname) = d.id().and_then(|id| pcfg.type_name_for_id(id)) else {
            continue;
        };
        for sec in &pcfg.types[tname].sections {
            match sec.kind {
                // A "validated checklist" (one carrying a scope=checked check) is
                // the new test plan: unchecked items are uncovered.
                SectionKind::Checklist
                    if sec.checks.iter().any(|c| c.scope == CheckScope::Checked) =>
                {
                    uncovered_testplan += body::checklist_items(&d.body, &sec.heading)
                        .iter()
                        .filter(|i| !i.checked)
                        .count();
                }
                SectionKind::Manual => {
                    for it in body::manual_items_in(&d.body, &sec.heading) {
                        manual_total += 1;
                        if it.uncovered() {
                            manual_uncovered += 1;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let (tag_keys, plain_tags) = tag_stats(docs);

    StatsReport {
        total: docs.len(),
        per_type,
        uncovered_testplan,
        manual_total,
        manual_uncovered,
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

    println!("\nuncovered test-plan items: {}", r.uncovered_testplan);
    println!("manual verification items: {}", r.manual_total);
    println!(
        "manual items without automated coverage: {}",
        r.manual_uncovered
    );
    Ok(())
}
