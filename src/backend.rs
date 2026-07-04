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
    /// (stats, history) and the TUI board.
    fn load_docs(&self, prj: &Project) -> (Vec<crate::doc::Doc>, Vec<String>);
}
