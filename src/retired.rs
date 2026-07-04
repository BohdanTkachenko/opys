//! The retired-id ledger: `_retired.md`, a frontmatter map of reserved
//! `id -> title`. Closing, retiring, renumbering, or deleting a document
//! records its id here so the global id sequence never hands the number out
//! again; the title is kept for human reference (git records the when and why).
//!
//! This supersedes the pre-0.12 plaintext `_retired.txt` (`ID  # retired …`).
//! A legacy ledger is read on load and migrated to `_retired.md` on the next
//! write. Reusing frontmatter + the `refs` id->title map machinery means one
//! less bespoke format to parse.

use std::path::{Path, PathBuf};

use crate::frontmatter::{self, Frontmatter};
use crate::refs;

/// The ledger filename.
pub const FILE: &str = "_retired.md";
/// The pre-0.12 plaintext ledger, migrated to [`FILE`] on the next write.
pub const LEGACY_FILE: &str = "_retired.txt";
/// The frontmatter key holding the reserved `id -> title` map.
pub const FIELD: &str = "retired";

const BODY: &str = "# Retired ids\n\nReserved ids that must never be reused. \
Managed by opys — the value is the document's last title; git records when and \
why each id was retired.\n";

/// Path to the markdown ledger under `base`.
pub fn path(base: &Path) -> PathBuf {
    base.join(FILE)
}

/// Path to the legacy plaintext ledger under `base`.
pub fn legacy_path(base: &Path) -> PathBuf {
    base.join(LEGACY_FILE)
}

/// The ledger as `(id, title)` pairs sorted by number. Prefers `_retired.md`;
/// falls back to the legacy plaintext ledger (ids only, empty titles). A missing
/// ledger reads as empty.
pub fn read(base: &Path) -> Vec<(String, String)> {
    if let Ok(text) = std::fs::read_to_string(path(base)) {
        if let Ok((fm, _)) = frontmatter::parse(&text, FILE) {
            return refs::parse_in(&fm, FIELD);
        }
    }
    read_legacy(base)
}

/// Parse the legacy plaintext ledger (`ID  # retired DATE: reason`): the id is
/// the first token of each non-comment line; titles are unknown (empty).
fn read_legacy(base: &Path) -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(legacy_path(base)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let head = line.split('#').next().unwrap_or("");
        if let Some(id) = head.split_whitespace().next() {
            if !id.is_empty() {
                out.push((id.to_string(), String::new()));
            }
        }
    }
    out
}

/// Render `(id, title)` entries (sorted by number) to the ledger file's text.
pub fn serialize(entries: &[(String, String)]) -> String {
    let mut fm = Frontmatter::new();
    refs::set_in(&mut fm, FIELD, entries);
    frontmatter::serialize(&fm, BODY)
}
