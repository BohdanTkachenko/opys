//! A single inventory document in memory — the unified representation of every
//! configured type. This collapses the former `Feature` and `WorkItem`, which
//! were identical `{path, frontmatter, body, title}` structs; a doc's *type* is
//! derived from its ID prefix via the config, not stored here.

use std::path::PathBuf;

use crate::body;
use crate::frontmatter::{self, Frontmatter};

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

    /// Serialized file text (canonical frontmatter + body).
    pub fn to_text(&self) -> String {
        frontmatter::serialize(&self.frontmatter, &self.body)
    }
}
