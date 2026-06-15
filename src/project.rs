//! The project: the inventory directory, its config, and feature discovery.
//!
//! The inventory lives under a base directory (default `docs/opys/`, relative
//! to the project root), holding `features/` (config + feature files +
//! INDEX.md), the optional `work-items/`, `views/`, and `runbooks/` as siblings.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use walkdir::WalkDir;

use crate::config::{Config, WorkItemConfig, FEAT_PREFIX};
use crate::doc::Doc;
use crate::error::{usage, OpysError, Result};
use crate::feature::Feature;
use crate::project_config::ProjectConfig;
use crate::refs;
use crate::work_item::WorkItem;

/// Resolve the inventory base directory from the project root and `dir`
/// (default `docs/opys`). An absolute `dir` is used as-is.
pub fn resolve_base(root: &str, dir: &str) -> PathBuf {
    let d = Path::new(dir);
    if d.is_absolute() {
        d.to_path_buf()
    } else {
        Path::new(root).join(d)
    }
}

pub struct Project {
    pub root: PathBuf,
    pub base: PathBuf,
    pub fdir: PathBuf,
    pub wdir: PathBuf,
    pub views_dir: PathBuf,
    pub runbooks_dir: PathBuf,
    pub cfg: Config,
    /// Work-item config; `None` when the subsystem is not configured.
    pub wi_cfg: Option<WorkItemConfig>,
    /// The universal engine config: the real `opys.toml` when present, else
    /// synthesized from the legacy config. Drives `verify`'s rule enforcement.
    pub pcfg: ProjectConfig,
}

impl Project {
    /// Open the project whose inventory base is `<root>/<dir>` (or an absolute
    /// `dir`). Requires `<base>/features/_config.toml`. The work-item subsystem
    /// is enabled when `<base>/work-items/_config.toml` also exists.
    pub fn open(root: &str, dir: &str) -> Result<Project> {
        let base = resolve_base(root, dir);
        let fdir = base.join("features");
        let wdir = base.join("work-items");
        let cfg = Config::load(&fdir.join("_config.toml"))?;
        let wi_cfg = WorkItemConfig::load_optional(&wdir.join("_config.toml"))?;
        let opys_toml = base.join("opys.toml");
        let pcfg = if opys_toml.exists() {
            ProjectConfig::load(&opys_toml)?
        } else {
            ProjectConfig::from_legacy(&cfg, wi_cfg.as_ref())
        };
        Ok(Project {
            root: PathBuf::from(root),
            views_dir: base.join("views"),
            runbooks_dir: base.join("runbooks"),
            base,
            fdir,
            wdir,
            cfg,
            wi_cfg,
            pcfg,
        })
    }

    /// The work-item config, or a usage error directing the user to configure
    /// the subsystem first.
    pub fn require_wi_cfg(&self) -> Result<&WorkItemConfig> {
        self.wi_cfg
            .as_ref()
            .ok_or_else(|| usage("work items not configured — run 'opys work-item init'"))
    }

    /// All feature files, sorted by path: `features/**/*.md`, excluding `_*`
    /// files and the generated `INDEX.md`.
    pub fn feature_paths(&self) -> Vec<PathBuf> {
        md_files(&self.fdir)
    }

    /// Load every feature, returning successfully parsed features and a list of
    /// parse-error messages (for `verify` / `sync-views`).
    pub fn load(&self) -> (Vec<Feature>, Vec<String>) {
        let mut feats = Vec::new();
        let mut errors = Vec::new();
        for p in self.feature_paths() {
            match std::fs::read_to_string(&p) {
                Ok(text) => match Feature::parse(p, &text) {
                    Ok(f) => feats.push(f),
                    Err(msg) => errors.push(msg),
                },
                Err(e) => errors.push(format!("{}: {e}", p.display())),
            }
        }
        (feats, errors)
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

    /// Highest numeric ID part used *anywhere* (`0` if none): across every live
    /// feature and work item, the retired-ID ledger, and every relation map (so
    /// a closed work item's struck tombstone still reserves its number). This is
    /// the basis for the single global, monotonically increasing ID sequence
    /// shared by features and all work-item types — so numbers never duplicate
    /// across prefixes.
    pub fn max_id_number(&self, feats: &[Feature], wis: &[WorkItem]) -> u64 {
        let mut max = 0u64;
        let mut consider = |id: &str| {
            if let Some(n) = id_part(id) {
                max = max.max(n);
            }
        };
        for f in feats {
            if let Some(id) = f.id() {
                consider(id);
            }
            for id in refs::all_relation_ids(&f.frontmatter) {
                consider(&id);
            }
        }
        for w in wis {
            if let Some(id) = w.id() {
                consider(id);
            }
            for id in refs::all_relation_ids(&w.frontmatter) {
                consider(&id);
            }
        }
        for id in self.retired_ids() {
            consider(&id);
        }
        max
    }

    /// Render a numeric ID part as a padded `FEAT-NNNN` id.
    pub fn format_id(&self, n: u64) -> String {
        format!("{}-{:0pad$}", FEAT_PREFIX, n, pad = self.cfg.pad)
    }

    /// Next feature ID: one past the highest number used anywhere (global).
    pub fn next_id(&self, feats: &[Feature], wis: &[WorkItem]) -> String {
        self.format_id(self.max_id_number(feats, wis) + 1)
    }

    pub fn path_for(&self, id: &str) -> PathBuf {
        self.fdir.join(format!("{id}.md"))
    }

    /// Find a feature by ID, or a usage error.
    pub fn find<'a>(&self, feats: &'a [Feature], id: &str) -> Result<&'a Feature> {
        feats
            .iter()
            .find(|f| f.id() == Some(id))
            .ok_or_else(|| OpysError::NotFound { id: id.to_string() })
    }

    pub fn find_mut<'a>(&self, feats: &'a mut [Feature], id: &str) -> Result<&'a mut Feature> {
        feats
            .iter_mut()
            .find(|f| f.id() == Some(id))
            .ok_or_else(|| OpysError::NotFound { id: id.to_string() })
    }

    // --- Work items -------------------------------------------------------

    /// All live work-item files, sorted by path (`work-items/**/*.md`,
    /// excluding `_*` and the generated `INDEX.md`).
    pub fn work_item_paths(&self) -> Vec<PathBuf> {
        md_files(&self.wdir)
    }

    /// Load every live work item, returning parsed items and parse errors.
    pub fn load_work_items(&self) -> (Vec<WorkItem>, Vec<String>) {
        let mut items = Vec::new();
        let mut errors = Vec::new();
        for p in self.work_item_paths() {
            match std::fs::read_to_string(&p) {
                Ok(text) => match WorkItem::parse(p, &text) {
                    Ok(w) => items.push(w),
                    Err(msg) => errors.push(msg),
                },
                Err(e) => errors.push(format!("{}: {e}", p.display())),
            }
        }
        (items, errors)
    }

    pub fn format_wi_id(&self, prefix: &str, n: u64) -> String {
        let pad = self.wi_cfg.as_ref().map(|c| c.pad).unwrap_or(4);
        format!("{}-{:0pad$}", prefix, n, pad = pad)
    }

    pub fn wi_path_for(&self, id: &str) -> PathBuf {
        self.wdir.join(format!("{id}.md"))
    }

    /// Next ID for a work-item `prefix`: one past the highest number used
    /// anywhere (the single global sequence), formatted with this type's prefix.
    /// Features and every work-item type draw from one increasing sequence, so
    /// numbers never collide across prefixes.
    pub fn next_id_for_prefix(&self, prefix: &str, feats: &[Feature], wis: &[WorkItem]) -> String {
        self.format_wi_id(prefix, self.max_id_number(feats, wis) + 1)
    }

    pub fn find_wi<'a>(&self, items: &'a [WorkItem], id: &str) -> Result<&'a WorkItem> {
        items
            .iter()
            .find(|w| w.id() == Some(id))
            .ok_or_else(|| OpysError::NotFound { id: id.to_string() })
    }

    pub fn find_wi_mut<'a>(&self, items: &'a mut [WorkItem], id: &str) -> Result<&'a mut WorkItem> {
        items
            .iter_mut()
            .find(|w| w.id() == Some(id))
            .ok_or_else(|| OpysError::NotFound { id: id.to_string() })
    }
}

/// Persist a document to disk in canonical form.
pub fn write_doc(d: &Doc) -> Result<()> {
    std::fs::write(&d.path, d.to_text()).map_err(OpysError::from)
}

/// Persist a feature to disk in canonical form.
pub fn write_feature(f: &Feature) -> Result<()> {
    std::fs::write(&f.path, f.to_text()).map_err(OpysError::from)
}

/// Persist a work item to disk in canonical form.
pub fn write_work_item(w: &WorkItem) -> Result<()> {
    std::fs::write(&w.path, w.to_text()).map_err(OpysError::from)
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
