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

/// A failure while rendering markdown from data via [`Schema::render`].
#[derive(Debug, Clone, thiserror::Error)]
pub enum RenderError {
    #[error("required field '{0}' is missing from the data")]
    MissingField(String),
    #[error("field '{field}' has wrong type: expected {expected}")]
    WrongType {
        field: String,
        expected: &'static str,
    },
}

/// A failure while querying a document via [`Schema::query`].
#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    #[error("document does not conform to the schema")]
    Validation(#[source] ValidationErrors),
    #[error("jq filter error: {0}")]
    Filter(String),
    #[error("value serialization error: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// Newtype wrapping a list of conformance problems so `QueryError` can derive
/// `Error` with a clean `#[source]`.
#[derive(Debug)]
pub struct ValidationErrors(pub Vec<Problem>);

impl std::fmt::Display for ValidationErrors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msgs: Vec<String> = self.0.iter().map(|p| p.to_string()).collect();
        write!(f, "{}", msgs.join("; "))
    }
}

impl std::error::Error for ValidationErrors {}

/// A failure while editing a document in-place via [`Schema::edit`].
#[derive(Debug, Clone, thiserror::Error)]
pub enum EditError {
    #[error("target path does not resolve to an editable node")]
    TargetNotFound,
    #[error("index {index} is out of range (list has {len} items)")]
    IndexOutOfRange { index: usize, len: usize },
}
