//! A single inventory document in memory — the unified representation of every
//! configured type. This collapses the former `Feature` and `WorkItem`, which
//! were identical `{path, frontmatter, body, title}` structs; a doc's *type* is
//! derived from its ID prefix via the config, not stored here.

use std::path::PathBuf;

use crate::body;
use crate::config::FEAT_PREFIX;
use crate::frontmatter::{self, Frontmatter};
use crate::refs;

#[derive(Debug, Clone)]
pub struct Doc {
    pub path: PathBuf,
    pub frontmatter: Frontmatter,
    pub body: String,
    pub title: String,
}

impl Doc {
    /// Parse a file's text into a `Doc`, or return the parse-error message.
    pub fn parse(path: PathBuf, text: &str) -> Result<Doc, String> {
        let display = path.display().to_string();
        let (frontmatter, body) = frontmatter::parse(text, &display).map_err(|e| e.0)?;
        let title = body::title(&body);
        Ok(Doc {
            path,
            frontmatter,
            body,
            title,
        })
    }

    pub fn id(&self) -> Option<&str> {
        self.frontmatter.id()
    }

    pub fn status(&self) -> Option<&str> {
        self.frontmatter.status()
    }

    /// IDs in the `references` map carrying the given prefix.
    pub fn refs_with_prefix(&self, prefix: &str) -> Vec<String> {
        refs::ids_with_prefix(&self.frontmatter, prefix)
    }

    /// Referenced feature IDs. Temporary FEAT-specific helper; callers migrate to
    /// [`Doc::refs_with_prefix`] as the type model generalizes.
    pub fn feature_refs(&self) -> Vec<String> {
        self.refs_with_prefix(FEAT_PREFIX)
    }

    /// Serialized file text (canonical frontmatter + body).
    pub fn to_text(&self) -> String {
        frontmatter::serialize(&self.frontmatter, &self.body)
    }
}
