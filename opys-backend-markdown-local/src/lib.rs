//! The markdown + local-filesystem backend for opys: one markdown file per
//! document, discovered and written under the inventory base. This is the
//! default (and, today, only) [`opys_engine::backend::Backend`] implementation. It owns
//! all corpus filesystem I/O — walking and parsing documents on load, and
//! executing the store's medium-agnostic [`FlushPlan`] on flush — so the core
//! `opys` crate performs no document filesystem access.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use walkdir::WalkDir;

use opys_engine::backend::Backend;
use opys_engine::doc::Doc;
use opys_engine::error::{OpysError, Result};
use opys_engine::project::Project;
use opys_engine::store::{FlushPlan, LoadedCorpus, Store};

/// The markdown + local-filesystem backend.
#[derive(Default)]
pub struct MarkdownLocal;

impl Backend for MarkdownLocal {
    fn load(&self, prj: &Project) -> Result<(Store, Vec<String>)> {
        let (docs, errors) = load_docs(prj);
        let docs: Vec<(Doc, Option<String>)> = docs
            .into_iter()
            .map(|d| {
                let mtime = mtime_rfc3339(&d.path);
                (d, mtime)
            })
            .collect();
        let retired = opys_engine::retired::read(&prj.base);
        let retired_legacy = opys_engine::retired::legacy_path(&prj.base).exists();
        Store::build(
            prj,
            LoadedCorpus {
                docs,
                errors,
                retired,
                retired_legacy,
            },
        )
    }

    fn flush(&self, prj: &Project, store: Store) -> Result<()> {
        let plan = store.flush_plan(prj)?;
        apply_plan(&plan, &prj.base)
    }

    fn load_docs(&self, prj: &Project) -> (Vec<Doc>, Vec<String>) {
        load_docs(prj)
    }
}

/// Read and parse every document file under the inventory base, sorted by path.
/// Unparsable files become non-fatal error messages (skipped).
pub fn load_docs(prj: &Project) -> (Vec<Doc>, Vec<String>) {
    let mut docs = Vec::new();
    let mut errors = Vec::new();
    for p in md_files(&prj.base) {
        match std::fs::read_to_string(&p) {
            Ok(text) => match Doc::parse(p, &text) {
                Ok(d) => docs.push(d),
                Err(msg) => errors.push(msg),
            },
            Err(e) => errors.push(format!("{}: {e}", p.display())),
        }
    }
    docs.sort_by(|a, b| a.path.cmp(&b.path));
    (docs, errors)
}

/// Write a single document to its canonical path, relocating (rename) first if
/// its id/status now imply a different path. Emptied source dirs are pruned.
/// Used by the TUI's inline editor.
pub fn save_doc(prj: &Project, d: &mut Doc) -> Result<()> {
    let target = match (d.id(), d.status()) {
        (Some(id), Some(status)) => Some(prj.doc_path(id, status)),
        _ => None,
    };
    if let Some(target) = target {
        if target != d.path {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let old = std::mem::replace(&mut d.path, target.clone());
            if old.exists() {
                std::fs::rename(&old, &target)?;
                prune_empty_dir(old.parent(), &prj.base);
            }
        }
    }
    std::fs::write(&d.path, d.to_text()).map_err(OpysError::from)
}

/// Execute a [`FlushPlan`] against the local filesystem: deletes → renames →
/// writes → ledger, pruning emptied document directories (never the base).
pub fn apply_plan(plan: &FlushPlan, base: &Path) -> Result<()> {
    for p in &plan.deletes {
        if p.exists() {
            std::fs::remove_file(p)?;
        }
        prune_empty_dir(p.parent(), base);
    }
    for (from, to) in &plan.renames {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if from.exists() {
            std::fs::rename(from, to)?;
            prune_empty_dir(from.parent(), base);
        }
    }
    for (target, text) in &plan.writes {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(target, text)?;
    }
    if let Some((path, text)) = &plan.retired_write {
        std::fs::write(path, text)?;
    }
    if let Some(legacy) = &plan.legacy_remove {
        std::fs::remove_file(legacy)?;
    }
    Ok(())
}

/// Document files anywhere under the base: filenames shaped like an id
/// (`PREFIX-NNNN.md`), sorted. Excludes `INDEX.md`, `_retired.*`, and stray
/// markdown.
fn md_files(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| DOC_FILENAME_RE.is_match(n))
        })
        .collect();
    paths.sort();
    paths
}

/// A document's original mtime as an rfc3339 string (for timestamp backfill).
fn mtime_rfc3339(path: &Path) -> Option<String> {
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;
    let mt = std::fs::metadata(path).ok()?.modified().ok()?;
    let dt = OffsetDateTime::from(mt);
    let dt = dt.replace_nanosecond(0).unwrap_or(dt);
    dt.format(&Rfc3339).ok()
}

/// Best-effort removal of an emptied document directory (never the base).
fn prune_empty_dir(dir: Option<&Path>, base: &Path) {
    if let Some(dir) = dir {
        if dir != base && dir.starts_with(base) {
            let _ = std::fs::remove_dir(dir); // no-op unless empty
        }
    }
}

/// A document filename: `PREFIX-NNNN.md`.
static DOC_FILENAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][A-Z0-9]*-[0-9]+\.md$").unwrap());
