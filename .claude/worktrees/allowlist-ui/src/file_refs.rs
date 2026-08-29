//! Scan source files for textual references to document ids.
//!
//! A document id like `FEAT-0123` is often mentioned in code — in comments,
//! string literals, identifiers. The project config (`[file_refs]`) declares the
//! textual *formats* an id may take (`{id}`, `{prefix}{num}`, `{prefix_lower}_{num}`,
//! …); this module renders each format for a set of ids, scans the configured
//! `roots`, and reports every hit with enough context to display it or to build a
//! `sed` fix command.
//!
//! This is deliberately separate from [`crate::refs`] (the `references` relation
//! maps *between documents*) — here the target is arbitrary source code.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use walkdir::{DirEntry, WalkDir};

use crate::project::Project;

/// One occurrence of a document id in a source file.
pub struct FileRef {
    /// The document id that matched (the canonical `PREFIX-NNNN`).
    pub id: String,
    /// The matched file, relative to the project root.
    pub path: PathBuf,
    /// 1-based line number.
    pub line: usize,
    /// The full matched line, trimmed of surrounding whitespace.
    pub text: String,
    /// The exact substring that matched (the rendered form of the id).
    pub matched: String,
    /// The format template that produced the match (see [`RefFormat`]).
    pub template: String,
    /// Whether the matching format required word boundaries.
    pub word: bool,
}

/// Render a [`RefFormat`] template for `id`, or `None` if `id` is malformed.
/// Placeholders: `{id}`, `{prefix}`, `{prefix_lower}`, `{num}`, `{padded}`.
pub fn render(template: &str, id: &str, pad: usize) -> Option<String> {
    let (prefix, numstr) = id.rsplit_once('-')?;
    let num: u64 = numstr.parse().ok()?;
    Some(
        template
            .replace("{prefix_lower}", &prefix.to_lowercase())
            .replace("{prefix}", prefix)
            .replace("{padded}", &format!("{num:0pad$}"))
            .replace("{num}", &num.to_string())
            .replace("{id}", id),
    )
}

/// Build the search regex for a rendered form: the literal text, optionally
/// wrapped in word boundaries.
fn search_re(rendered: &str, word: bool) -> Option<regex::Regex> {
    let escaped = regex::escape(rendered);
    let pat = if word {
        format!(r"\b{escaped}\b")
    } else {
        escaped
    };
    regex::Regex::new(&pat).ok()
}

/// Scan the configured `roots` for textual references to any of `ids`. Returns
/// hits sorted by (path, line, id). Each id is matched in every effective format.
pub fn scan(prj: &Project, ids: &[&str]) -> Vec<FileRef> {
    let pad = prj.pcfg.pad;
    let formats = prj.pcfg.file_refs.effective_formats();

    // Precompute (id, rendered text, regex, template, word) for every id × format.
    struct Needle {
        id: String,
        rendered: String,
        re: regex::Regex,
        template: String,
        word: bool,
    }
    let mut needles: Vec<Needle> = Vec::new();
    for id in ids {
        for f in &formats {
            let Some(rendered) = render(&f.template, id, pad) else {
                continue;
            };
            let Some(re) = search_re(&rendered, f.word) else {
                continue;
            };
            needles.push(Needle {
                id: id.to_string(),
                rendered,
                re,
                template: f.template.clone(),
                word: f.word,
            });
        }
    }
    if needles.is_empty() {
        return Vec::new();
    }

    let mut hits: Vec<FileRef> = Vec::new();
    for abs in list_files(prj, &prj.pcfg.file_refs.roots) {
        let Ok(bytes) = std::fs::read(&abs) else {
            continue;
        };
        // Skip files that aren't valid UTF-8 (binaries) rather than lossily
        // scanning them.
        let Ok(content) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let rel = abs.strip_prefix(&prj.root).unwrap_or(&abs).to_path_buf();
        for (i, line) in content.lines().enumerate() {
            for n in &needles {
                if !line.contains(&n.rendered) {
                    continue; // cheap pre-filter before the regex
                }
                if n.re.is_match(line) {
                    hits.push(FileRef {
                        id: n.id.clone(),
                        path: rel.clone(),
                        line: i + 1,
                        text: line.trim().to_string(),
                        matched: n.rendered.clone(),
                        template: n.template.clone(),
                        word: n.word,
                    });
                }
            }
        }
    }

    hits.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(a.line.cmp(&b.line))
            .then(a.id.cmp(&b.id))
    });
    hits
}

/// A `sed -i` command that rewrites every occurrence of one id's rendered form to
/// the new id's rendered form in `path`. `word` mirrors the format so the
/// substitution matches exactly what the scan found.
pub fn sed_fix(rel_path: &Path, old: &str, new: &str, word: bool) -> String {
    // Ids are `[A-Za-z0-9_-]`, none of which are special to a `/`-delimited `sed`
    // s/// command, so no escaping is needed. Word boundaries map to GNU sed `\b`.
    let pat = if word {
        format!(r"\b{old}\b")
    } else {
        old.to_string()
    };
    format!("sed -i 's/{pat}/{new}/g' {}", rel_path.display())
}

/// Enumerate the files to scan under `roots` (project-root relative), skipping
/// the inventory base, `.git`, `target`, `node_modules`, and hidden directories.
fn list_files(prj: &Project, roots: &[String]) -> Vec<PathBuf> {
    let base = prj.base.as_path();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out: Vec<PathBuf> = Vec::new();
    for root in roots {
        let start = prj.root.join(root);
        for entry in WalkDir::new(&start)
            .into_iter()
            .filter_entry(|e| !skip_dir(e, base))
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                let p = entry.into_path();
                if seen.insert(p.clone()) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// Whether a walked directory should be pruned from the scan.
fn skip_dir(e: &DirEntry, base: &Path) -> bool {
    if !e.file_type().is_dir() {
        return false;
    }
    // Never prune the scan root itself — a project may legitimately live in a
    // hidden directory (e.g. a temp dir named `.tmpXXXX`), and `roots = ["."]`
    // normalizes back to that dir's name.
    if e.depth() == 0 {
        return false;
    }
    let p = e.path();
    if p == base {
        return true; // the inventory itself — its ids live there by design
    }
    match p.file_name().and_then(|s| s.to_str()) {
        Some(".git") | Some("target") | Some("node_modules") => true,
        // Hidden directories (`.foo`), but never the scan root given as ".".
        Some(name) => name.starts_with('.') && name.len() > 1,
        None => false,
    }
}
