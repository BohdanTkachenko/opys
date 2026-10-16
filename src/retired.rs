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

use crate::error::{usage, OpysError, Result};
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
/// falls back to the legacy plaintext ledger (ids only, empty titles) only when
/// the markdown ledger is *absent*. A missing ledger reads as empty; a present
/// but unreadable/unparseable one is an error — treating it as empty would
/// silently release every reserved id (and the next reservation would rewrite
/// the file without them).
pub fn read(base: &Path) -> Result<Vec<(String, String)>> {
    let p = path(base);
    let text = match std::fs::read_to_string(&p) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return read_legacy(base),
        Err(e) => return Err(corrupt(&p, &e.to_string())),
    };
    let (fm, _) = frontmatter::parse(&text, FILE).map_err(|e| corrupt(&p, &e.0))?;
    match fm.get(FIELD) {
        Some(serde_norway::Value::Mapping(_)) => Ok(refs::parse_in(&fm, FIELD)),
        _ => Err(corrupt(&p, "no 'retired' id map in the frontmatter")),
    }
}

/// The error for a present-but-unreadable ledger.
fn corrupt(path: &Path, why: &str) -> OpysError {
    usage(format!(
        "{}: unreadable retired ledger ({why}) — fix or restore the file; \
         reserved ids must never read as empty",
        path.display()
    ))
}

/// Parse the legacy plaintext ledger (`ID  # retired DATE: reason`): the id is
/// the first token of each non-comment line; titles are unknown (empty). Like
/// the markdown ledger, a present-but-unreadable file is an error — an empty
/// read here would both release the reservations and let the next flush's
/// migration delete the file outright.
fn read_legacy(base: &Path) -> Result<Vec<(String, String)>> {
    let p = legacy_path(base);
    let text = match std::fs::read_to_string(&p) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(corrupt(&p, &e.to_string())),
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
    Ok(out)
}

/// Render `(id, title)` entries (sorted by number) to the ledger file's text.
pub fn serialize(entries: &[(String, String)]) -> String {
    let mut fm = Frontmatter::new();
    refs::set_in(&mut fm, FIELD, entries);
    frontmatter::serialize(&fm, BODY)
}
