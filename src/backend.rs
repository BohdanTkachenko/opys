//! The storage backend seam.
//!
//! A [`Backend`] is the durable medium the corpus lives in: it loads documents
//! into the in-memory working [`Store`] and flushes the store's net changes
//! back. Commands operate on the core `Store` and never touch the medium
//! directly — they go through the injected backend, so the medium is swappable.
//!
//! [`MarkdownLocal`] is the default (and, today, only) implementation: one
//! markdown file per document in a local-filesystem inventory. Other media (a
//! database, a remote service) would be alternative `Backend` impls, selected by
//! the binary.

use crate::error::Result;
use crate::project::Project;
use crate::store::Store;

/// A durable storage medium for the corpus.
pub trait Backend {
    /// Load the durable corpus into a fresh working [`Store`], plus non-fatal
    /// parse errors (unparsable documents are skipped, not fatal).
    fn load(&self, prj: &Project) -> Result<(Store, Vec<String>)>;

    /// Persist the store's net changes: write changed documents, delete removed
    /// ones, relocate moved ones, and rewrite the retired-id ledger.
    fn flush(&self, prj: &Project, store: Store) -> Result<()>;

    /// Read and parse every durable document (raw, without building a store),
    /// returning parsed docs + non-fatal parse errors. Used by read-only passes
    /// (stats, history).
    fn load_docs(&self, prj: &Project) -> (Vec<crate::doc::Doc>, Vec<String>);

    /// The document at `path` exactly as the medium stores it, when the medium
    /// stores documents as text at all; `None` otherwise, and `None` when the
    /// text can no longer be read (the store relocated the document in memory,
    /// say).
    ///
    /// This is the *spelling*, not the model: parsing normalizes quoting away,
    /// so the one check that needs the source — format.md's quoted `history`
    /// keys, see `commands::verify::spelling_problems` — reaches the medium
    /// through here rather than reading files behind the backend's back. A
    /// backend that returns `None` simply does not run it.
    fn stored_text(&self, path: &std::path::Path) -> Option<String>;
}
