use thiserror::Error;

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("Index error: {0}")]
    Index(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Decode error: {0}")]
    Decode(#[from] prost::DecodeError),
}

impl From<tantivy::TantivyError> for StoreError {
    fn from(e: tantivy::TantivyError) -> Self {
        StoreError::Storage(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;
