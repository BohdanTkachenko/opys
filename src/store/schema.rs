//! DDL for the in-memory corpus database.
//!
//! Two layers live in one DB:
//!
//! - **Authoritative** tables (`docs`, `tags`, `relations`, `fm_fields`,
//!   `retired`) hold full document fidelity: every frontmatter key has exactly
//!   one home (see `decompose`), and the original file text/path/mtime are
//!   retained so `flush` can diff instead of rewriting.
//! - **Derived** tables (`fields`, `sections`) are the stable projection user
//!   SQL queries (same shapes as the `[[stats]]` contract); they are rebuilt
//!   from scratch by `refresh_projections` and never written by commands.

/// All CREATE TABLE statements, executed once at [`super::Store::open`].
pub(crate) const DDL: &str = "\
CREATE TABLE docs (\
  dkey INTEGER, id TEXT, num INTEGER, type TEXT, status TEXT, title TEXT, \
  created TEXT, updated TEXT, body TEXT, path TEXT, \
  orig_path TEXT, orig_text TEXT, orig_mtime TEXT);
CREATE TABLE tags (dkey INTEGER, doc_id TEXT, seq INTEGER, tag TEXT, key TEXT, value TEXT);
CREATE TABLE relations (\
  dkey INTEGER, doc_id TEXT, field TEXT, seq INTEGER, ref_id TEXT, ref_num INTEGER, \
  raw_value TEXT, title TEXT, struck BOOLEAN);
CREATE TABLE fm_fields (dkey INTEGER, doc_id TEXT, key TEXT, value_yaml TEXT, value TEXT, kind TEXT);
CREATE TABLE retired (rkey INTEGER, id TEXT, num INTEGER, title TEXT);
CREATE TABLE fields (doc_id TEXT, key TEXT, value TEXT);
CREATE TABLE sections (doc_id TEXT, heading TEXT, kind TEXT, items INTEGER, unchecked INTEGER);
";
