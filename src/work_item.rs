//! Re-export shim: a work item is now a [`crate::doc::Doc`]. Kept temporarily so
//! existing `use crate::work_item::WorkItem` paths resolve while the two-family
//! model is collapsed into one document type.

pub use crate::doc::Doc as WorkItem;
