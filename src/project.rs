//! The project: the inventory directory, its config, and feature discovery.
//!
//! The inventory lives under a base directory (default `docs/`, relative to
//! the project root), holding `features/` (config + feature files + INDEX.md),
//! `views/`, and `runbooks/` as siblings.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use walkdir::WalkDir;

use crate::config::Config;
use crate::error::{usage, OpysError, Result};
use crate::feature::Feature;

/// Resolve the inventory base directory from the project root and `dir`
/// (default `docs`). An absolute `dir` is used as-is.
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
    pub views_dir: PathBuf,
    pub runbooks_dir: PathBuf,
    pub cfg: Config,
}

impl Project {
    /// Open the project whose inventory base is `<root>/<dir>` (or an absolute
    /// `dir`). Requires `<base>/features/_config.toml`.
    pub fn open(root: &str, dir: &str) -> Result<Project> {
        let base = resolve_base(root, dir);
        let fdir = base.join("features");
        let cfg = Config::load(&fdir.join("_config.toml"))?;
        Ok(Project {
            root: PathBuf::from(root),
            views_dir: base.join("views"),
            runbooks_dir: base.join("runbooks"),
            base,
            fdir,
            cfg,
        })
    }

    /// All feature files, sorted by path: `features/**/*.md`, excluding `_*`
    /// files and the generated `INDEX.md`.
    pub fn feature_paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = WalkDir::new(&self.fdir)
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

    /// IDs that have been retired and may never be reused.
    pub fn retired_ids(&self) -> HashSet<String> {
        let rp = self.fdir.join("_retired.txt");
        let Ok(text) = std::fs::read_to_string(&rp) else {
            return HashSet::new();
        };
        let mut out = HashSet::new();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if let Some(first) = line.split_whitespace().next() {
                out.insert(first.to_string());
            }
        }
        out
    }

    /// Highest numeric ID part across live and retired IDs (`0` if none),
    /// the basis for allocating the next ID(s).
    pub fn max_id_number(&self, feats: &[Feature]) -> u64 {
        let re = id_number_re(&self.cfg.prefix);
        let mut max = 0u64;
        let live = feats.iter().filter_map(|f| f.id().map(str::to_string));
        for id in live.chain(self.retired_ids()) {
            if let Some(c) = re.captures(&id) {
                if let Ok(n) = c[1].parse::<u64>() {
                    max = max.max(n);
                }
            }
        }
        max
    }

    /// Render a numeric ID part as a padded `PREFIX-NNNN` id.
    pub fn format_id(&self, n: u64) -> String {
        format!("{}-{:0pad$}", self.cfg.prefix, n, pad = self.cfg.pad)
    }

    /// Next ID: max numeric part across live and retired IDs, plus one.
    pub fn next_id(&self, feats: &[Feature]) -> String {
        self.format_id(self.max_id_number(feats) + 1)
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
}

/// Persist a feature to disk in canonical form.
pub fn write_feature(f: &Feature) -> Result<()> {
    std::fs::write(&f.path, f.to_text()).map_err(OpysError::from)
}

fn id_number_re(prefix: &str) -> Regex {
    Regex::new(&format!(r"^{}-(\d+)$", regex::escape(prefix))).unwrap()
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
