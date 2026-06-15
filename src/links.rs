//! Cross-reference auto-sync: keep every doc's `references` map bidirectional
//! and title-fresh, and rewrite bare `FEAT-XXXX`/`WI-XXXX` mentions in body
//! prose into readable markdown links. Run by `sync` after every mutating
//! command.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::{Captures, Regex};

use crate::feature::Feature;
use crate::refs;
use crate::work_item::WorkItem;

/// Either an existing opys markdown link or a bare ID mention. The prefix
/// alternation is built from the live ID prefixes (feature + every work-item
/// type) so new types are linkified without touching this regex.
static REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    let alt = std::iter::once(crate::config::FEAT_PREFIX)
        .chain(crate::config::work_item_prefixes())
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join("|");
    Regex::new(&format!(
        r"\[(?P<lid>(?:{alt})-[0-9]+)[^\]]*\]\([^)]*\)|\b(?P<bid>(?:{alt})-[0-9]+)\b"
    ))
    .unwrap()
});

/// Reconcile every doc's `references` map: make links bidirectional between
/// present docs and refresh each value to the referenced doc's current title.
/// A reference whose target is absent (a closed work item's struck tombstone,
/// or a dangling id) keeps its existing value untouched.
pub fn reconcile(feats: &mut [Feature], live: &mut [WorkItem]) {
    let mut title: HashMap<String, String> = HashMap::new();
    let mut existing: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut out: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut present: HashSet<String> = HashSet::new();

    let docs = feats
        .iter()
        .map(|f| (f.id(), &f.title, &f.frontmatter))
        .chain(live.iter().map(|w| (w.id(), &w.title, &w.frontmatter)));
    for (id, t, fm) in docs {
        let Some(id) = id else { continue };
        present.insert(id.to_string());
        title.insert(id.to_string(), t.clone());
        let mut ex = HashMap::new();
        let set = out.entry(id.to_string()).or_default();
        for (tid, val) in refs::parse(fm) {
            set.insert(tid.clone());
            ex.insert(tid, val);
        }
        existing.insert(id.to_string(), ex);
    }

    // Make edges bidirectional between present docs.
    let mut desired = out.clone();
    for (src, targets) in &out {
        for t in targets {
            if present.contains(t) {
                desired.entry(t.clone()).or_default().insert(src.clone());
            }
        }
    }

    let compute = |id: &str| -> Vec<(String, String)> {
        desired
            .get(id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|tid| {
                let value = title.get(&tid).cloned().unwrap_or_else(|| {
                    existing
                        .get(id)
                        .and_then(|m| m.get(&tid))
                        .cloned()
                        .unwrap_or_default()
                });
                (tid, value)
            })
            .collect()
    };

    for f in feats.iter_mut() {
        if let Some(id) = f.id().map(str::to_string) {
            refs::set(&mut f.frontmatter, &compute(&id));
        }
    }
    for w in live.iter_mut() {
        if let Some(id) = w.id().map(str::to_string) {
            refs::set(&mut w.frontmatter, &compute(&id));
        }
    }
}

/// Reconcile the directional blocker relation: `blocked_by` on one doc and
/// `blocks` on the other are kept as inverses, with titles refreshed from the
/// referenced doc. An edge is asserted if *either* endpoint records it (union
/// semantics, like [`reconcile`]); removal is therefore done on both sides by
/// the `unblock` command. Edges whose other endpoint is absent (a struck
/// tombstone or a dangling id) keep their existing value untouched.
pub fn reconcile_blockers(feats: &mut [Feature], live: &mut [WorkItem]) {
    let mut title: HashMap<String, String> = HashMap::new();
    let mut present: HashSet<String> = HashSet::new();
    // existing[id][field] = (target -> stored value), for the absent-target fallback.
    let mut existing: HashMap<String, HashMap<&'static str, HashMap<String, String>>> =
        HashMap::new();
    // Canonical directed edges (blocker, blocked), unioned from both fields.
    let mut edges: HashSet<(String, String)> = HashSet::new();

    let docs = feats
        .iter()
        .map(|f| (f.id(), &f.title, &f.frontmatter))
        .chain(live.iter().map(|w| (w.id(), &w.title, &w.frontmatter)));
    for (id, t, fm) in docs {
        let Some(id) = id else { continue };
        present.insert(id.to_string());
        title.insert(id.to_string(), t.clone());
        let mut by_field = HashMap::new();
        let mut bb = HashMap::new();
        for (b, val) in refs::parse_in(fm, refs::BLOCKED_BY) {
            edges.insert((b.clone(), id.to_string()));
            bb.insert(b, val);
        }
        by_field.insert(refs::BLOCKED_BY, bb);
        let mut bk = HashMap::new();
        for (a, val) in refs::parse_in(fm, refs::BLOCKS) {
            edges.insert((id.to_string(), a.clone()));
            bk.insert(a, val);
        }
        by_field.insert(refs::BLOCKS, bk);
        existing.insert(id.to_string(), by_field);
    }

    // Materialize each edge on whichever present doc owns that side.
    let mut desired_bb: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut desired_bk: HashMap<String, BTreeSet<String>> = HashMap::new();
    for (blocker, blocked) in &edges {
        if present.contains(blocked) {
            desired_bb
                .entry(blocked.clone())
                .or_default()
                .insert(blocker.clone());
        }
        if present.contains(blocker) {
            desired_bk
                .entry(blocker.clone())
                .or_default()
                .insert(blocked.clone());
        }
    }

    let compute = |id: &str, field: &'static str, want: &HashMap<String, BTreeSet<String>>| {
        want.get(id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|tid| {
                let value = title.get(&tid).cloned().unwrap_or_else(|| {
                    existing
                        .get(id)
                        .and_then(|m| m.get(field))
                        .and_then(|m| m.get(&tid))
                        .cloned()
                        .unwrap_or_default()
                });
                (tid, value)
            })
            .collect::<Vec<_>>()
    };

    for f in feats.iter_mut() {
        if let Some(id) = f.id().map(str::to_string) {
            refs::set_in(
                &mut f.frontmatter,
                refs::BLOCKED_BY,
                &compute(&id, refs::BLOCKED_BY, &desired_bb),
            );
            refs::set_in(
                &mut f.frontmatter,
                refs::BLOCKS,
                &compute(&id, refs::BLOCKS, &desired_bk),
            );
        }
    }
    for w in live.iter_mut() {
        if let Some(id) = w.id().map(str::to_string) {
            refs::set_in(
                &mut w.frontmatter,
                refs::BLOCKED_BY,
                &compute(&id, refs::BLOCKED_BY, &desired_bb),
            );
            refs::set_in(
                &mut w.frontmatter,
                refs::BLOCKS,
                &compute(&id, refs::BLOCKS, &desired_bk),
            );
        }
    }
}

/// Map of every present doc's ID to (title, file path), for linkifying bodies.
pub fn build_index(feats: &[Feature], live: &[WorkItem]) -> HashMap<String, (String, PathBuf)> {
    let mut idx = HashMap::new();
    for f in feats {
        if let Some(id) = f.id() {
            idx.insert(id.to_string(), (f.title.clone(), f.path.clone()));
        }
    }
    for w in live {
        if let Some(id) = w.id() {
            idx.insert(id.to_string(), (w.title.clone(), w.path.clone()));
        }
    }
    idx
}

/// Rewrite bare `FEAT-XXXX`/`WI-XXXX` mentions (and refresh existing opys
/// links) in body prose into `[ID — Title](relpath)`. Idempotent; references
/// inside fenced code blocks or inline code spans are left untouched, and a
/// mention whose target is absent from `index` is left as-is.
pub fn linkify(
    body: &str,
    current_dir: &Path,
    index: &HashMap<String, (String, PathBuf)>,
) -> String {
    let mut out = String::new();
    let mut in_fence = false;
    for (i, line) in body.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        // Split on backticks: even segments are prose, odd are inline code.
        let mut code = false;
        for (j, seg) in line.split('`').enumerate() {
            if j > 0 {
                out.push('`');
            }
            if code {
                out.push_str(seg);
            } else {
                out.push_str(&linkify_prose(seg, current_dir, index));
            }
            code = !code;
        }
    }
    out
}

fn linkify_prose(
    seg: &str,
    current_dir: &Path,
    index: &HashMap<String, (String, PathBuf)>,
) -> String {
    REF_RE
        .replace_all(seg, |c: &Captures| {
            let id = c
                .name("lid")
                .or_else(|| c.name("bid"))
                .map(|m| m.as_str())
                .unwrap_or_default();
            match index.get(id) {
                Some((title, path)) => {
                    format!("[{id} — {title}]({})", relpath(current_dir, path))
                }
                None => c.get(0).map(|m| m.as_str()).unwrap_or_default().to_string(),
            }
        })
        .into_owned()
}

/// Relative path from a directory to a target file, using `/` separators.
fn relpath(from_dir: &Path, to: &Path) -> String {
    let from: Vec<_> = from_dir.components().collect();
    let to_c: Vec<_> = to.components().collect();
    let mut i = 0;
    while i < from.len() && i < to_c.len() && from[i] == to_c[i] {
        i += 1;
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in i..from.len() {
        parts.push("..".to_string());
    }
    for c in &to_c[i..] {
        parts.push(c.as_os_str().to_string_lossy().into_owned());
    }
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx() -> HashMap<String, (String, PathBuf)> {
        let mut m = HashMap::new();
        m.insert(
            "FEAT-0001".to_string(),
            (
                "Auth login".to_string(),
                PathBuf::from("/p/features/FEAT-0001.md"),
            ),
        );
        m
    }

    #[test]
    fn linkifies_bare_id_and_is_idempotent() {
        let dir = Path::new("/p/work-items");
        let once = linkify("See FEAT-0001 for context.", dir, &idx());
        assert_eq!(
            once,
            "See [FEAT-0001 — Auth login](../features/FEAT-0001.md) for context."
        );
        let twice = linkify(&once, dir, &idx());
        assert_eq!(twice, once);
    }

    #[test]
    fn skips_code_spans_and_unknown_ids() {
        let dir = Path::new("/p/work-items");
        let body = "Inline `FEAT-0001` stays, FEAT-9999 unknown stays.";
        assert_eq!(linkify(body, dir, &idx()), body);
    }

    #[test]
    fn skips_fenced_blocks() {
        let dir = Path::new("/p/work-items");
        let body = "```\nFEAT-0001\n```";
        assert_eq!(linkify(body, dir, &idx()), body);
    }
}
