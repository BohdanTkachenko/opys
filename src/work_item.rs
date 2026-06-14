//! A single work-item file in memory — the ephemeral, implementation-tracking
//! companion to one or more features. Mirrors [`crate::feature::Feature`] but
//! is a distinct type with its own reserved fields and lifecycle.

use std::path::PathBuf;

use crate::body;
use crate::config::FEAT_PREFIX;
use crate::frontmatter::{self, Frontmatter};
use crate::refs;

#[derive(Debug, Clone)]
pub struct WorkItem {
    pub path: PathBuf,
    pub frontmatter: Frontmatter,
    pub body: String,
    pub title: String,
}

impl WorkItem {
    /// Parse a file's text into a `WorkItem`, or return the parse-error message.
    pub fn parse(path: PathBuf, text: &str) -> Result<WorkItem, String> {
        let display = path.display().to_string();
        let (frontmatter, body) = frontmatter::parse(text, &display).map_err(|e| e.0)?;
        let title = body::title(&body);
        Ok(WorkItem {
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

    /// Referenced feature IDs (the required link(s) into the inventory).
    pub fn feature_refs(&self) -> Vec<String> {
        refs::ids_with_prefix(&self.frontmatter, FEAT_PREFIX)
    }

    /// Serialized file text (canonical frontmatter + body).
    pub fn to_text(&self) -> String {
        frontmatter::serialize(&self.frontmatter, &self.body)
    }
}
