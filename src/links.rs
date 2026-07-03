//! Cross-reference auto-sync: keep every doc's `references` map bidirectional
//! and title-fresh, and rewrite bare `FEAT-XXXX`/`WI-XXXX` mentions in body
//! prose into readable markdown links. Run by `sync` after every mutating
//! command.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use regex::{Captures, Regex};

use crate::doc::Doc;
use crate::refs;

/// Build the bare-ID matcher whose prefix alternation comes from the live
/// configured type prefixes, so new types are linkified automatically.
pub fn ref_re(prefixes: &[String]) -> Regex {
    let alt = prefixes
        .iter()
        .map(|p| regex::escape(p))
        .collect::<Vec<_>>()
        .join("|");
    Regex::new(&format!(r"\b(?:{alt})-[0-9]+\b")).unwrap()
}

/// Reconcile every doc's `references` map: make links bidirectional between
/// present docs and refresh each value to the referenced doc's current title.
/// A reference whose target is absent (a closed work item's struck tombstone,
/// or a dangling id) keeps its existing value untouched.
pub fn reconcile(docs: &mut [Doc]) {
    let mut title: HashMap<String, String> = HashMap::new();
    let mut existing: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut out: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut present: HashSet<String> = HashSet::new();

    for (id, t, fm) in docs.iter().map(|d| (d.id(), &d.title, &d.frontmatter)) {
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

    for d in docs.iter_mut() {
        if let Some(id) = d.id().map(str::to_string) {
            refs::set(&mut d.frontmatter, &compute(&id));
        }
    }
}

/// Reconcile the directional blocker relation: `blocked_by` on one doc and
/// `blocks` on the other are kept as inverses, with titles refreshed from the
/// referenced doc. An edge is asserted if *either* endpoint records it (union
/// semantics, like [`reconcile`]); removal is therefore done on both sides by
/// the `unblock` command. Edges whose other endpoint is absent (a struck
/// tombstone or a dangling id) keep their existing value untouched.
pub fn reconcile_blockers(docs: &mut [Doc]) {
    let mut title: HashMap<String, String> = HashMap::new();
    let mut present: HashSet<String> = HashSet::new();
    // existing[id][field] = (target -> stored value), for the absent-target fallback.
    let mut existing: HashMap<String, HashMap<&'static str, HashMap<String, String>>> =
        HashMap::new();
    // Canonical directed edges (blocker, blocked), unioned from both fields.
    let mut edges: HashSet<(String, String)> = HashSet::new();

    for (id, t, fm) in docs.iter().map(|d| (d.id(), &d.title, &d.frontmatter)) {
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

    for d in docs.iter_mut() {
        if let Some(id) = d.id().map(str::to_string) {
            refs::set_in(
                &mut d.frontmatter,
                refs::BLOCKED_BY,
                &compute(&id, refs::BLOCKED_BY, &desired_bb),
            );
            refs::set_in(
                &mut d.frontmatter,
                refs::BLOCKS,
                &compute(&id, refs::BLOCKS, &desired_bk),
            );
        }
    }
}

/// Map of every present doc's ID to (title, file path), for linkifying bodies.
pub fn build_index(docs: &[Doc]) -> HashMap<String, (String, PathBuf)> {
    let mut idx = HashMap::new();
    for d in docs {
        if let Some(id) = d.id() {
            idx.insert(id.to_string(), (d.title.clone(), d.path.clone()));
        }
    }
    idx
}

/// Rewrite bare `FEAT-XXXX`/`WI-XXXX` mentions (and refresh existing opys
/// links) in body prose into `[ID — Title](relpath)`. Idempotent; references
/// inside fenced code blocks, inline code spans, or existing markdown link
/// syntax (label or destination) are left untouched, and a mention whose
/// target is absent from `index` is left as-is.
pub fn linkify(
    body: &str,
    current_dir: &Path,
    index: &HashMap<String, (String, PathBuf)>,
    re: &Regex,
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
                out.push_str(&linkify_prose(seg, current_dir, index, re));
            }
            code = !code;
        }
    }
    out
}

/// Linkify one prose segment (no code spans/fences). Existing markdown links
/// are parsed with bracket-depth counting — labels may themselves contain
/// `[…]` — and their interior is never re-matched, so a link is never nested
/// inside another. A link whose label starts with a live ID is refreshed to
/// the current title and path; every other link passes through verbatim.
fn linkify_prose(
    seg: &str,
    current_dir: &Path,
    index: &HashMap<String, (String, PathBuf)>,
    re: &Regex,
) -> String {
    let mut out = String::new();
    let mut pos = 0;
    while let Some(off) = seg[pos..].find('[') {
        let start = pos + off;
        match parse_link(seg, start) {
            Some((label, end)) => {
                out.push_str(&replace_bare(&seg[pos..start], current_dir, index, re));
                let is_image = seg[..start].ends_with('!');
                let refreshed = if is_image {
                    None
                } else {
                    re.find(label)
                        .filter(|m| m.start() == 0)
                        .and_then(|m| index.get(m.as_str()).map(|t| (m.as_str(), t)))
                        .map(|(id, (title, path))| {
                            format!("[{id} — {title}]({})", relpath(current_dir, path))
                        })
                };
                match refreshed {
                    Some(link) => out.push_str(&link),
                    None => out.push_str(&seg[start..end]),
                }
                pos = end;
            }
            None => {
                // A `[` that opens no link (e.g. a checkbox `[ ]`) is prose.
                out.push_str(&replace_bare(&seg[pos..start], current_dir, index, re));
                out.push('[');
                pos = start + 1;
            }
        }
    }
    out.push_str(&replace_bare(&seg[pos..], current_dir, index, re));
    out
}

/// Parse a markdown link `[label](dest)` starting at `start` (a `[`). Returns
/// the label and the index one past the closing `)`. Brackets in the label and
/// parens in the destination are matched by depth, so bracketed titles round-trip.
fn parse_link(s: &str, start: usize) -> Option<(&str, usize)> {
    let rest = &s[start..];
    let mut depth = 0usize;
    let mut label_end = None;
    for (i, ch) in rest.char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    label_end = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let label_end = label_end?;
    let after = &rest[label_end + 1..];
    if !after.starts_with('(') {
        return None;
    }
    let mut pdepth = 0usize;
    for (i, ch) in after.char_indices() {
        match ch {
            '(' => pdepth += 1,
            ')' => {
                pdepth -= 1;
                if pdepth == 0 {
                    return Some((&rest[1..label_end], start + label_end + 1 + i + 1));
                }
            }
            _ => {}
        }
    }
    None
}

/// Replace bare ID mentions in link-free prose with fresh markdown links.
fn replace_bare(
    seg: &str,
    current_dir: &Path,
    index: &HashMap<String, (String, PathBuf)>,
    re: &Regex,
) -> String {
    re.replace_all(seg, |c: &Captures| {
        let id = &c[0];
        match index.get(id) {
            Some((title, path)) => {
                format!("[{id} — {title}]({})", relpath(current_dir, path))
            }
            None => id.to_string(),
        }
    })
    .into_owned()
}

/// Scan a body for nested markdown links — a complete `[…](…)` inside another
/// link's label. That is invalid markdown and, in an opys inventory, the
/// footprint of linkify corruption; `verify` flags each occurrence. Fenced code
/// blocks and inline code spans are skipped, like in [`linkify`]. Returns one
/// truncated snippet per offending link.
pub fn nested_links(body: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut in_fence = false;
    for line in body.split('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        for (j, seg) in line.split('`').enumerate() {
            if j % 2 == 1 {
                continue; // inline code span
            }
            let mut pos = 0;
            while let Some(off) = seg[pos..].find('[') {
                let start = pos + off;
                match parse_link(seg, start) {
                    Some((label, end)) => {
                        if contains_link(label) {
                            found.push(snippet(&seg[start..end]));
                        }
                        pos = end;
                    }
                    None => pos = start + 1,
                }
            }
        }
    }
    found
}

/// Whether `s` contains a complete markdown link anywhere.
fn contains_link(s: &str) -> bool {
    let mut pos = 0;
    while let Some(off) = s[pos..].find('[') {
        let start = pos + off;
        if parse_link(s, start).is_some() {
            return true;
        }
        pos = start + 1;
    }
    false
}

/// First characters of `s`, with an ellipsis when truncated.
fn snippet(s: &str) -> String {
    const MAX: usize = 60;
    if s.chars().count() <= MAX {
        s.to_string()
    } else {
        let cut: String = s.chars().take(MAX).collect();
        format!("{cut}…")
    }
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
        let re = ref_re(&["FEAT".to_string()]);
        let once = linkify("See FEAT-0001 for context.", dir, &idx(), &re);
        assert_eq!(
            once,
            "See [FEAT-0001 — Auth login](../features/FEAT-0001.md) for context."
        );
        let twice = linkify(&once, dir, &idx(), &re);
        assert_eq!(twice, once);
    }

    #[test]
    fn skips_code_spans_and_unknown_ids() {
        let dir = Path::new("/p/work-items");
        let body = "Inline `FEAT-0001` stays, FEAT-9999 unknown stays.";
        assert_eq!(
            linkify(body, dir, &idx(), &ref_re(&["FEAT".to_string()])),
            body
        );
    }

    #[test]
    fn bracketed_link_labels_are_not_relinkified() {
        // Regression: a label containing `[…]` (e.g. a title like
        // "[Light] / [Dark] dual-variant support") used to be re-matched by
        // the bare-ID branch, nesting the link deeper on every sync.
        let dir = Path::new("/p/work-items");
        let re = ref_re(&["FEAT".to_string()]);
        let mut m = HashMap::new();
        m.insert(
            "FEAT-0315".to_string(),
            (
                "[Light] / [Dark] dual-variant support".to_string(),
                PathBuf::from("/p/features/FEAT-0315.md"),
            ),
        );
        let once = linkify("See FEAT-0315.", dir, &m, &re);
        assert_eq!(
            once,
            "See [FEAT-0315 — [Light] / [Dark] dual-variant support](../features/FEAT-0315.md)."
        );
        let twice = linkify(&once, dir, &m, &re);
        assert_eq!(twice, once);
    }

    #[test]
    fn refreshes_stale_link_title_and_path() {
        let dir = Path::new("/p/work-items");
        let re = ref_re(&["FEAT".to_string()]);
        let stale = "See [FEAT-0001 — Old name](old/FEAT-0001.md).";
        assert_eq!(
            linkify(stale, dir, &idx(), &re),
            "See [FEAT-0001 — Auth login](../features/FEAT-0001.md)."
        );
    }

    #[test]
    fn ids_inside_foreign_link_syntax_stay_untouched() {
        let dir = Path::new("/p/work-items");
        let re = ref_re(&["FEAT".to_string()]);
        let body = "See [notes on FEAT-0001](notes.md) and ![FEAT-0001 shot](FEAT-0001.png).";
        assert_eq!(linkify(body, dir, &idx(), &re), body);
    }

    #[test]
    fn checkbox_lines_still_linkify() {
        let dir = Path::new("/p/work-items");
        let re = ref_re(&["FEAT".to_string()]);
        assert_eq!(
            linkify("- [ ] Cover FEAT-0001", dir, &idx(), &re),
            "- [ ] Cover [FEAT-0001 — Auth login](../features/FEAT-0001.md)"
        );
    }

    #[test]
    fn nested_links_are_detected() {
        let body = "Ok [FEAT-0001 — Auth login](../features/FEAT-0001.md) here.\n\
                    Bad [[FEAT-0315 — [Light] mode](FEAT-0315.md) — extra](FEAT-0315.md) there.";
        let found = nested_links(body);
        assert_eq!(found.len(), 1);
        assert!(found[0].starts_with("[[FEAT-0315"), "snippet: {}", found[0]);
    }

    #[test]
    fn nested_links_skip_code_and_bracketed_labels() {
        let body = "`[[a](b)](c)` inline code\n\
                    ```\n[[a](b)](c)\n```\n\
                    [FEAT-0315 — [Light] / [Dark] support](FEAT-0315.md) plain brackets";
        assert!(nested_links(body).is_empty());
    }

    #[test]
    fn skips_fenced_blocks() {
        let dir = Path::new("/p/work-items");
        let body = "```\nFEAT-0001\n```";
        assert_eq!(
            linkify(body, dir, &idx(), &ref_re(&["FEAT".to_string()])),
            body
        );
    }
}
