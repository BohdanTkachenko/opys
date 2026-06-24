//! `opys renumber` — reassign IDs to documents that conflict with the base
//! branch (same numeric part, different full IDs), keeping the IDs that already
//! existed at the git merge-base and renumbering the ones that were added on a
//! feature branch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::commands::{maybe_sync, today};
use crate::doc::Doc;
use crate::error::Result;
use crate::project::{self, Project};
use crate::Ctx;

pub fn run(ctx: &Ctx, base: Option<&str>) -> Result<()> {
    let prj = ctx.open()?;
    let (docs, _) = prj.load_docs();

    let conflicts = find_conflicts(&docs);
    if conflicts.is_empty() {
        println!("renumber: no conflicting IDs found");
        return Ok(());
    }

    let (git_root, base_sha) = git_context(&prj.root, base);
    match &base_sha {
        Some(sha) => println!("renumber: base {}", &sha[..sha.len().min(12)]),
        None => {
            eprintln!("renumber: warning: no git base found — keeping first of each conflict group")
        }
    }

    // Build old→new mapping, allocating new IDs sequentially past the current max.
    let max = prj.max_doc_id(&docs);
    let mut counter = max + 1;
    let mut mapping: HashMap<String, String> = HashMap::new();

    for group in &conflicts {
        let mut keep: Vec<&str> = Vec::new();
        let mut to_renumber: Vec<&str> = Vec::new();

        for id in group {
            let at_base = docs
                .iter()
                .find(|d| d.id() == Some(id.as_str()))
                .is_some_and(|d| exists_at_base(&git_root, &base_sha, d));
            if at_base {
                keep.push(id.as_str());
            } else {
                to_renumber.push(id.as_str());
            }
        }

        // If nothing is from the base (two feature branches both added an ID),
        // keep the first alphabetically and renumber the rest.
        let start = if keep.is_empty() { 1 } else { 0 };
        for id in &to_renumber[start..] {
            let prefix = id.split('-').next().unwrap_or("FEAT");
            let new_id = format!("{}-{:0pad$}", prefix, counter, pad = prj.pcfg.pad);
            println!("  {id} → {new_id}");
            mapping.insert(id.to_string(), new_id);
            counter += 1;
        }
    }

    if mapping.is_empty() {
        println!("renumber: all conflicting IDs are from the base branch, nothing to do");
        return Ok(());
    }

    // Text-level substitution across every doc file, then rename files whose
    // own ID changed.
    for doc in &docs {
        let text = std::fs::read_to_string(&doc.path)?;
        let new_text = apply_renames(&text, &mapping);
        if new_text == text {
            continue;
        }
        let old_id = doc.id().unwrap_or("");
        if let Some(new_id) = mapping.get(old_id) {
            let status = doc.status().unwrap_or("");
            let new_path = prj.doc_path(new_id, status);
            if let Some(parent) = new_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&new_path, &new_text)?;
            std::fs::remove_file(&doc.path)?;
        } else {
            std::fs::write(&doc.path, &new_text)?;
        }
    }

    // Retire old IDs so they are never reallocated.
    let rp = prj.base.join("_retired.txt");
    for old_id in mapping.keys() {
        let line = format!("{old_id}  # renumbered {}", today());
        project::write_id_ledger_entry(&rp, old_id, &line)?;
    }

    println!("renumber: {} document(s) renumbered", mapping.len());
    maybe_sync(ctx, &prj);

    warn_file_references(&prj, &mapping);
    Ok(())
}

/// After a successful renumber, scan code for references to the *old* ids (which
/// renumber did not rewrite — it only touches documents) and warn, with a `sed`
/// suggestion per file so the user or an agent can fix them selectively.
fn warn_file_references(prj: &Project, mapping: &HashMap<String, String>) {
    let pad = prj.pcfg.pad;
    let old_ids: Vec<&str> = mapping.keys().map(String::as_str).collect();
    let hits = crate::file_refs::scan(prj, &old_ids);
    if hits.is_empty() {
        return;
    }

    eprintln!(
        "\nwarning: {} file reference(s) still point at a renumbered id:",
        hits.len()
    );
    // Preserve first-seen order while de-duplicating identical fix commands.
    let mut fixes: Vec<String> = Vec::new();
    for h in &hits {
        let new_id = mapping.get(&h.id).map(String::as_str).unwrap_or(&h.id);
        eprintln!(
            "  {}:{}: {}  ({} → {})",
            h.path.display(),
            h.line,
            h.text,
            h.id,
            new_id
        );
        if let Some(new_form) = crate::file_refs::render(&h.template, new_id, pad) {
            let cmd = crate::file_refs::sed_fix(&h.path, &h.matched, &new_form, h.word);
            if !fixes.contains(&cmd) {
                fixes.push(cmd);
            }
        }
    }
    if !fixes.is_empty() {
        eprintln!("\nsuggested fixes (review before running):");
        for cmd in &fixes {
            eprintln!("  {cmd}");
        }
    }
}

/// Groups of IDs that share the same numeric part (≥2 members, sorted).
fn find_conflicts(docs: &[Doc]) -> Vec<Vec<String>> {
    let mut by_num: HashMap<u64, Vec<String>> = HashMap::new();
    for doc in docs {
        if let Some(id) = doc.id() {
            if let Some(n) = id.rsplit_once('-').and_then(|(_, n)| n.parse::<u64>().ok()) {
                by_num.entry(n).or_default().push(id.to_string());
            }
        }
    }
    let mut groups: Vec<Vec<String>> = by_num
        .into_values()
        .filter(|g| g.len() > 1)
        .map(|mut g| {
            g.sort();
            g
        })
        .collect();
    groups.sort();
    groups
}

/// Replace old IDs with new IDs throughout `text`. Longer IDs are substituted
/// first to avoid partial-match issues when one ID is a prefix of another.
fn apply_renames(text: &str, mapping: &HashMap<String, String>) -> String {
    let mut pairs: Vec<_> = mapping.iter().collect();
    pairs.sort_by_key(|p| std::cmp::Reverse(p.0.len()));
    let mut out = text.to_string();
    for (old, new) in pairs {
        out = out.replace(old.as_str(), new.as_str());
    }
    out
}

// ---- git helpers ------------------------------------------------------------

fn git_context(root: &Path, base: Option<&str>) -> (Option<PathBuf>, Option<String>) {
    let git_root = git_toplevel(root);
    let base_sha = resolve_base(root, base);
    (git_root, base_sha)
}

fn git_toplevel(root: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(root)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

/// The base commit SHA: the explicit `--base` ref if given, otherwise the
/// merge-base of HEAD with main/master, or HEAD^1 (pre-merge tip) as a last
/// resort.
fn resolve_base(root: &Path, base: Option<&str>) -> Option<String> {
    if let Some(b) = base {
        let out = Command::new("git")
            .args(["rev-parse", b])
            .current_dir(root)
            .output()
            .ok()?;
        if out.status.success() {
            return Some(String::from_utf8_lossy(&out.stdout).trim().to_string());
        }
    }
    for branch in &["main", "master", "origin/main", "origin/master"] {
        let out = Command::new("git")
            .args(["merge-base", "HEAD", branch])
            .current_dir(root)
            .output()
            .ok()?;
        if out.status.success() {
            return Some(String::from_utf8_lossy(&out.stdout).trim().to_string());
        }
    }
    // Fall back to the first parent (pre-merge main tip).
    let out = Command::new("git")
        .args(["rev-parse", "HEAD^1"])
        .current_dir(root)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// True if `doc`'s file exists in the git tree at `base_sha`.
fn exists_at_base(git_root: &Option<PathBuf>, base_sha: &Option<String>, doc: &Doc) -> bool {
    let (Some(git_root), Some(sha)) = (git_root, base_sha) else {
        return false;
    };
    let Ok(rel) = doc.path.strip_prefix(git_root) else {
        return false;
    };
    Command::new("git")
        .args(["cat-file", "-e", &format!("{sha}:{}", rel.display())])
        .current_dir(git_root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
