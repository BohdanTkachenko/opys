use std::path::PathBuf;

/// Errors raised by library operations.
///
/// Content problems found by `verify` are *not* errors — they are collected
/// into a list and reported with a dedicated exit code. These variants cover
/// usage mistakes and runtime/IO failures.
#[derive(Debug, thiserror::Error)]
pub enum OpysError {
    #[error("{0} not found — run 'opys init' first")]
    ConfigNotFound(PathBuf),

    #[error("{id} not found")]
    NotFound { id: String },

    /// A usage mistake (bad flags, failed guard, etc.). Mirrors the Python
    /// tool's `sys.exit("error: …")` cases.
    #[error("{0}")]
    Usage(String),

    #[error("{path}: {source}")]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Convenience for raising a usage error.
pub fn usage(msg: impl Into<String>) -> OpysError {
    OpysError::Usage(msg.into())
}

pub type Result<T> = std::result::Result<T, OpysError>;
