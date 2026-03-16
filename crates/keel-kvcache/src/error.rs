use thiserror::Error;

#[derive(Error, Debug)]
pub enum KvError {
    /// Filesystem or mmap I/O failure (memmap2 errors are std::io::Error).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Index error: {0}")]
    Index(String),
}

pub type Result<T> = std::result::Result<T, KvError>;
