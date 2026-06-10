//! A single feature file in memory.

use std::path::PathBuf;

use crate::body;
use crate::frontmatter::{self, Frontmatter};

#[derive(Debug, Clone)]
pub struct Feature {
    pub path: PathBuf,
    pub frontmatter: Frontmatter,
    pub body: String,
    pub title: String,
}

impl Feature {
    /// Parse a file's text into a `Feature`, or return the parse-error message.
    pub fn parse(path: PathBuf, text: &str) -> Result<Feature, String> {
        let display = path.display().to_string();
        let (frontmatter, body) = frontmatter::parse(text, &display).map_err(|e| e.0)?;
        let title = body::title(&body);
        Ok(Feature {
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
