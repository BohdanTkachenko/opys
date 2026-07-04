//! The project: `opys.toml` at the project root (found by searching upward),
//! plus the inventory base it points at (default `opys/`, holding the
//! document files and `_retired.txt`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

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

    /// The canonical on-disk path for a document with the given id and status,
    /// per the configured `layout` (see [`ProjectConfig::doc_relpath`]).
    pub fn doc_path(&self, id: &str, status: &str) -> PathBuf {
        self.base.join(self.pcfg.doc_relpath(id, status))
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
        crate::retired::read(&self.base)
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    /// Find a document by ID, or a not-found error.
    pub fn find<'a>(&self, docs: &'a [Doc], id: &str) -> Result<&'a Doc> {
        docs.iter()
            .find(|d| d.id() == Some(id))
            .ok_or_else(|| OpysError::NotFound { id: id.to_string() })
    }
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
