//! The board: the loaded documents plus reload semantics (last-good on a total
//! parse failure, so a mid-write race never blanks the screen).

use opys_engine::backend::Backend;
use opys_engine::doc::Doc;
use opys_engine::project::Project;

use super::sort::{sort_docs, SortState};

pub struct Board {
    pub docs: Vec<Doc>,
    pub errors: Vec<String>,
}

impl Board {
    pub fn load(prj: &Project, sort: SortState) -> Board {
        let (mut docs, errors) = opys_backend_markdown_local::MarkdownLocal.load_docs(prj);
        sort_docs(&mut docs, sort);
        Board { docs, errors }
    }

    /// Reload from disk and re-sort. If the load yields no documents but does
    /// yield errors (a parse failure caught mid-write), keep the last-good set
    /// and just surface the errors — never blank a working board on a transient.
    pub fn reload(&mut self, prj: &Project, sort: SortState) {
        let (mut docs, errors) = opys_backend_markdown_local::MarkdownLocal.load_docs(prj);
        if docs.is_empty() && !errors.is_empty() {
            self.errors = errors;
            return;
        }
        sort_docs(&mut docs, sort);
        self.docs = docs;
        self.errors = errors;
    }
}
