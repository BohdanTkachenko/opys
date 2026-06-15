//! Re-export shim: a feature is now a [`crate::doc::Doc`]. Kept temporarily so
//! existing `use crate::feature::Feature` paths resolve while the two-family
//! model is collapsed into one document type.

pub use crate::doc::Doc as Feature;
