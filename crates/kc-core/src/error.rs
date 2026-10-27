use std::path::PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("invalid request: {0}")]
    BadRequest(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("operation cancelled")]
    Cancelled,

    #[error("timed out: {0}")]
    Timeout(String),

    #[error("path {path} escapes workspace root {root}")]
    PathEscape { path: PathBuf, root: PathBuf },

    #[error("provider error: {0}")]
    Provider(String),

    #[error("tool error: {0}")]
    Tool(String),

    #[error("storage error: {0}")]
    Store(String),

    #[error("plugin error: {0}")]
    Plugin(String),

    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    /// Stable machine-readable code used by the HTTP layer and the UI.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Config(_) => "config",
            Error::BadRequest(_) => "bad_request",
            Error::NotFound(_) => "not_found",
            Error::PermissionDenied(_) => "permission_denied",
            Error::Unauthorized => "unauthorized",
            Error::Cancelled => "cancelled",
            Error::Timeout(_) => "timeout",
            Error::PathEscape { .. } => "path_escape",
            Error::Provider(_) => "provider",
            Error::Tool(_) => "tool",
            Error::Store(_) => "store",
            Error::Plugin(_) => "plugin",
            Error::Io { .. } => "io",
            Error::Internal(_) => "internal",
        }
    }

    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Internal(format!("json: {e}"))
    }
}
