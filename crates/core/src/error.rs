use std::path::PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network error: {0}")]
    Http(String),

    #[error("the version server returned an error: {0}")]
    Api(String),

    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("checksum mismatch for {path}: expected {expected}, got {actual}")]
    Checksum {
        path: PathBuf,
        expected: String,
        actual: String,
    },

    #[error("cancelled")]
    Cancelled,

    #[error("failed to extract game files: {0}")]
    Extract(String),

    #[error("YooAsset: {0}")]
    Yoo(String),

    #[error("{0}")]
    Steam(String),

    #[error("could not parse {file}: {message}")]
    Vdf { file: String, message: String },
}

impl From<ureq::Error> for Error {
    fn from(e: ureq::Error) -> Self {
        Error::Http(e.to_string())
    }
}

/// Attach a human-readable context to io errors: `fs::read(p).io_ctx(|| format!(...))`.
pub trait IoContext<T> {
    fn io_ctx<S: Into<String>>(self, ctx: impl FnOnce() -> S) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn io_ctx<S: Into<String>>(self, ctx: impl FnOnce() -> S) -> Result<T> {
        self.map_err(|source| Error::Io {
            context: ctx().into(),
            source,
        })
    }
}
