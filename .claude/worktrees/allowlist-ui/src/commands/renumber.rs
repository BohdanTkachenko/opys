//! `opys renumber` — reassign IDs to documents that conflict with the base
//! branch (same numeric part, different full IDs), keeping the IDs that already
//! existed at the git merge-base and renumbering the ones that were added on a
//! feature branch.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use regex::{Captures, Regex};

use crate::commands::maybe_sync;
use crate::doc::Doc;
use crate::error::Result;
use crate::project::Project;
use crate::Ctx;

pub fn run(ctx: &Ctx, base: Option<&str>) -> Result<()> {
    let prj = ctx.open()?;
    let (mut store, _) = ctx.load(&prj)?;
    let docs: Vec<(i64, Doc)> = store.all_docs()?;

    let just_docs: Vec<&Doc> = docs.iter().map(|(_, d)| d).collect();
    let conflicts = find_conflicts(&just_docs);
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

    // The set of ids present at the base commit, resolved *by id* (the id-named
    // file anywhere under the inventory base) — not by current path, so a doc
    // that existed at base but was relocated on the branch is still recognized.
    let base_ids = ids_at_base(&git_root, &base_sha, &prj);

    // Build old→new mapping, allocating new IDs sequentially past the current max.
    let mut counter = store.max_doc_num()? + 1;
    let mut mapping: HashMap<String, String> = HashMap::new();

    for group in &conflicts {
        let mut keep: Vec<&str> = Vec::new();
        let mut to_renumber: Vec<&str> = Vec::new();

        for id in group {
            if base_ids.contains(id.as_str()) {
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

    // Text-level substitution across every doc (frontmatter AND body — an id may
    // appear in a custom field, a relation value, or prose): reconstruct the
    // full text, apply_renames, reparse, and write back through the store. A doc
    // whose own id changed gets a new canonical path (flush renames it).
    for (dkey, doc) in &docs {
        let text = doc.to_text();
        let new_text = apply_renames(&text, &mapping);
        if new_text == text {
            continue;
        }
        let new_doc = Doc::parse(doc.path.clone(), &new_text).map_err(crate::error::usage)?;
        store.put_doc(&prj.pcfg, Some(*dkey), &new_doc)?;
        store.set_canonical_path(&prj.pcfg, *dkey)?;
    }

    // Retire old IDs so they are never reallocated.
    for (old_id, new_id) in &mapping {
        let title = store.title_of(new_id)?.unwrap_or_default();
        store.retire_id(old_id, &title)?;
    }
    ctx.flush(&prj, store)?;

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
fn find_conflicts(docs: &[&Doc]) -> Vec<Vec<String>> {
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

/// Replace old IDs with new IDs throughout `text`, matching only whole ids
/// (word-bounded, so renaming `FEAT-1000` never corrupts `FEAT-10005`) and never
/// inside fenced code blocks (which linkify likewise skips). The alternation is
/// longest-first so a longer id wins when several share a prefix.
fn apply_renames(text: &str, mapping: &HashMap<String, String>) -> String {
    let mut olds: Vec<&String> = mapping.keys().collect();
    olds.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let alt = olds
        .iter()
        .map(|o| regex::escape(o))
        .collect::<Vec<_>>()
        .join("|");
    let re = Regex::new(&format!(r"\b(?:{alt})\b")).expect("valid id alternation");

    let mut out = String::new();
    let mut fence: Option<(char, usize)> = None;
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if let Some((fc, flen)) = fence {
            if crate::links::closes_fence(line, fc, flen) {
                fence = None;
            }
            out.push_str(line);
            continue;
        }
        if let Some((c, len)) = crate::links::opens_fence(line) {
            fence = Some((c, len));
            out.push_str(line);
            continue;
        }
        let replaced = re.replace_all(line, |c: &Captures| {
            mapping
                .get(&c[0])
                .cloned()
                .unwrap_or_else(|| c[0].to_string())
        });
        out.push_str(&replaced);
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

/// The document ids present at `base_sha`, resolved by the id-named file
/// (`<ID>.md`) anywhere under the inventory base — one `git ls-tree` for the
/// whole set. Location-independent, so a relocation on the branch (e.g. into
/// `_archived/`) does not make a base document look new.
fn ids_at_base(
    git_root: &Option<PathBuf>,
    base_sha: &Option<String>,
    prj: &Project,
) -> HashSet<String> {
    let (Some(git_root), Some(sha)) = (git_root, base_sha) else {
        return HashSet::new();
    };
    let mut args = vec![
        "ls-tree".to_string(),
        "-r".to_string(),
        "--name-only".to_string(),
        sha.clone(),
    ];
    // Scope to the inventory base when it lives under the git root.
    if let Ok(rel) = prj.base.strip_prefix(git_root) {
        args.push("--".to_string());
        args.push(rel.display().to_string());
    }
    let Ok(out) = Command::new("git")
        .args(&args)
        .current_dir(git_root)
        .output()
    else {
        return HashSet::new();
    };
    if !out.status.success() {
        return HashSet::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            Path::new(line)
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".md"))
                .map(str::to_string)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn apply_renames_matches_whole_ids_only() {
        let m = map(&[("FEAT-1000", "FEAT-2000")]);
        // The exact id is renamed; a longer id that merely shares the prefix
        // (once ids outgrow the pad width) is left intact.
        assert_eq!(
            apply_renames("see FEAT-1000 and FEAT-10005 here", &m),
            "see FEAT-2000 and FEAT-10005 here"
        );
    }

    #[test]
    fn apply_renames_skips_fenced_code() {
        let m = map(&[("FEAT-1000", "FEAT-2000")]);
        let text = "FEAT-1000 prose\n```\nFEAT-1000 code\n```\nFEAT-1000 tail";
        assert_eq!(
            apply_renames(text, &m),
            "FEAT-2000 prose\n```\nFEAT-1000 code\n```\nFEAT-2000 tail"
        );
    }

    #[test]
    fn apply_renames_rewrites_relation_values_and_prose() {
        let m = map(&[("TASK-0001", "TASK-0009")]);
        let text = "references:\n  TASK-0001: Wire it\n---\nSee TASK-0001 for context.";
        assert_eq!(
            apply_renames(text, &m),
            "references:\n  TASK-0009: Wire it\n---\nSee TASK-0009 for context."
        );
    }
}
