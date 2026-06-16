//! The project: `opys.toml` at the project root (found by searching upward),
//! plus the inventory base it points at (default `opys/`, holding the
//! document files, `INDEX.md`, and `_retired.txt`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use walkdir::WalkDir;

use crate::doc::Doc;
use crate::error::{usage, OpysError, Result};
use crate::project_config::ProjectConfig;
use crate::refs;

/// The directory to start the `opys.toml` search from: `root` made absolute
/// (default `.` → the current working directory).
pub fn start_dir(root: &str) -> Result<PathBuf> {
    let p = Path::new(root);
    if p.is_absolute() {
        Ok(p.to_path_buf())
    } else {
        Ok(std::env::current_dir().map_err(OpysError::from)?.join(p))
    }
}

/// Walk up from `start` (inclusive) to the filesystem root, returning the first
/// directory that contains an `opys.toml` — the project root.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    let mut cur = start.to_path_buf();
    loop {
        if cur.join("opys.toml").is_file() {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

pub struct Project {
    pub root: PathBuf,
    pub base: PathBuf,
    /// The universal engine config (`<root>/opys.toml`), the sole source of
    /// truth for document types, statuses, fields, sections, and rules.
    pub pcfg: ProjectConfig,
}

impl Project {
    /// Open the project by searching upward from `root` (default the cwd) for
    /// `opys.toml`. The directory holding it is the project root; the inventory
    /// base is `<root>/<config.base>` (default `opys`).
    pub fn open(root: &str) -> Result<Project> {
        let start = start_dir(root)?;
        let root = find_root(&start).ok_or_else(|| {
            usage(format!(
                "no opys.toml found in {} or any parent directory — run `opys init`",
                start.display()
            ))
        })?;
        let pcfg = ProjectConfig::load(&root.join("opys.toml"))?;
        let base = root.join(&pcfg.base);
        Ok(Project { base, root, pcfg })
    }

    /// Generic discovery: load every document across all configured type
    /// directories (`ProjectConfig::doc_dirs`, resolved under the base) as one
    /// global inventory, returning parsed docs (sorted by path) and parse-error
    /// messages. A doc's type is derived from its id prefix where needed.
    pub fn load_docs(&self) -> (Vec<Doc>, Vec<String>) {
        let mut docs = Vec::new();
        let mut errors = Vec::new();
        for dir in self.pcfg.doc_dirs() {
            for p in md_files(&self.base.join(dir)) {
                match std::fs::read_to_string(&p) {
                    Ok(text) => match Doc::parse(p, &text) {
                        Ok(d) => docs.push(d),
                        Err(msg) => errors.push(msg),
                    },
                    Err(e) => errors.push(format!("{}: {e}", p.display())),
                }
            }
        }
        docs.sort_by(|a, b| a.path.cmp(&b.path));
        (docs, errors)
    }

    /// Highest numeric id part across a document set, their relation maps, and
    /// the retired ledger — the basis for the single global, monotonically
    /// increasing id sequence shared by every type.
    pub fn max_doc_id(&self, docs: &[Doc]) -> u64 {
        let mut max = 0u64;
        let mut consider = |id: &str| {
            if let Some(n) = id_part(id) {
                max = max.max(n);
            }
        };
        for d in docs {
            if let Some(id) = d.id() {
                consider(id);
            }
            for id in refs::all_relation_ids(&d.frontmatter) {
                consider(&id);
            }
        }
        for id in self.retired_ids() {
            consider(&id);
        }
        max
    }

    /// Next id for a type `prefix`: one past the global max, padded to `pcfg.pad`.
    pub fn next_id_for(&self, prefix: &str, docs: &[Doc]) -> String {
        format!(
            "{}-{:0pad$}",
            prefix,
            self.max_doc_id(docs) + 1,
            pad = self.pcfg.pad
        )
    }

    /// IDs that have been retired and may never be reused.
    pub fn retired_ids(&self) -> HashSet<String> {
        read_id_ledger(&self.base.join("_retired.txt"))
    }

    /// Find a document by ID, or a not-found error.
    pub fn find<'a>(&self, docs: &'a [Doc], id: &str) -> Result<&'a Doc> {
        docs.iter()
            .find(|d| d.id() == Some(id))
            .ok_or_else(|| OpysError::NotFound { id: id.to_string() })
    }

    pub fn find_mut<'a>(&self, docs: &'a mut [Doc], id: &str) -> Result<&'a mut Doc> {
        docs.iter_mut()
            .find(|d| d.id() == Some(id))
            .ok_or_else(|| OpysError::NotFound { id: id.to_string() })
    }
}

/// Persist a document to disk in canonical form.
pub fn write_doc(d: &Doc) -> Result<()> {
    std::fs::write(&d.path, d.to_text()).map_err(OpysError::from)
}

/// Markdown files directly relevant to an inventory dir: `**/*.md` excluding
/// `_*` files and the generated `INDEX.md`, sorted by path.
fn md_files(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| {
            p.extension().is_some_and(|x| x == "md")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| !n.starts_with('_') && n != "INDEX.md")
        })
        .collect();
    paths.sort();
    paths
}

/// Parse a sorted-by-number ID ledger (`_retired.txt`), returning the IDs.
fn read_id_ledger(path: &Path) -> HashSet<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashSet::new();
    };
    let mut out = HashSet::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("");
        if let Some(first) = line.split_whitespace().next() {
            out.insert(first.to_string());
        }
    }
    out
}

/// Append an entry to an ID ledger, keeping the file sorted by item number.
pub fn write_id_ledger_entry(path: &Path, id: &str, line: &str) -> Result<()> {
    let mut entries: Vec<(u64, String)> = Vec::new();
    if let Ok(text) = std::fs::read_to_string(path) {
        for l in text.lines() {
            if l.trim().is_empty() {
                continue;
            }
            let eid = l
                .split('#')
                .next()
                .unwrap_or("")
                .split_whitespace()
                .next()
                .unwrap_or("");
            entries.push((refs::id_number(eid), l.to_string()));
        }
    }
    entries.push((refs::id_number(id), line.to_string()));
    entries.sort_by_key(|e| e.0);
    let mut out = String::new();
    for (_, l) in entries {
        out.push_str(&l);
        out.push('\n');
    }
    std::fs::write(path, out).map_err(OpysError::from)
}

/// The numeric part of a `PREFIX-NNNN` id, if it parses; `None` for malformed
/// ids (which the global-sequence max ignores rather than treating as huge).
fn id_part(id: &str) -> Option<u64> {
    id.rsplit_once('-').and_then(|(_, n)| n.parse::<u64>().ok())
}

/// `^PREFIX-\d{pad,}$` — the verify-time id format (pad-or-more digits).
pub fn id_format_re(prefix: &str, pad: usize) -> Regex {
    Regex::new(&format!(r"^{}-\d{{{pad},}}$", regex::escape(prefix))).unwrap()
}

pub static KEBAB_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9]+(-[a-z0-9]+)*$").unwrap());

/// Parse a `key=value` custom-field assignment, coercing the value through
/// YAML (so `n=3` is an int, `t=[a, b]` a list, `s=foo` a string).
pub fn parse_field(arg: &str) -> Result<(String, serde_norway::Value)> {
    let (k, v) = arg
        .split_once('=')
        .ok_or_else(|| usage(format!("--field expects key=value, got {arg:?}")))?;
    let value: serde_norway::Value = serde_norway::from_str(v.trim())
        .unwrap_or_else(|_| serde_norway::Value::String(v.trim().to_string()));
    Ok((k.trim().to_string(), value))
}
