use std::io;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid archive: {0}")]
    InvalidArchive(String),
    #[error("unsupported archive feature: {0}")]
    Unsupported(String),
    #[error("decompression failed: {0}")]
    Decompress(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("unsafe path '{0}' (would escape the output folder)")]
    UnsafePath(String),
}

impl Error {
    pub(crate) fn invalid(msg: impl Into<String>) -> Self {
        Error::InvalidArchive(msg.into())
    }
}
