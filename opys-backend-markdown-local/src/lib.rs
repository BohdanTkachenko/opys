//! The markdown + local-filesystem backend for opys: one markdown file per
//! document, discovered and written under the inventory base. This is the
//! default (and, today, only) [`opys::backend::Backend`] implementation; it
//! delegates to the core corpus store's load/flush over the local filesystem.

use opys::backend::Backend;
use opys::error::Result;
use opys::project::Project;
use opys::store::Store;

/// The markdown + local-filesystem backend.
#[derive(Default)]
pub struct MarkdownLocal;

impl Backend for MarkdownLocal {
    fn load(&self, prj: &Project) -> Result<(Store, Vec<String>)> {
        Store::open(prj)
    }

    fn flush(&self, prj: &Project, store: Store) -> Result<()> {
        store.flush(prj)
    }
}
