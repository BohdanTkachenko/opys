//! Error types: schema-parse errors (located) and document-validation problems.

/// A failure while parsing the DSL schema source. Carries a 1-based line/column.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("schema:{line}:{col}: {message}")]
pub struct SchemaError {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

impl SchemaError {
    pub(crate) fn new(line: usize, col: usize, message: impl Into<String>) -> Self {
        SchemaError {
            line,
            col,
            message: message.into(),
        }
    }
}

/// One way a document failed to conform, addressed by the breadcrumb `path`
/// (alias chain) of the schema node that was unmet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub path: Vec<String>,
    pub message: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{}: {}", self.path.join(" › "), self.message)
        }
    }
}
