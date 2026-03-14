// Unified Error type for keel

use thiserror::Error;

#[derive(Error, Debug)]
pub enum KeelError {
    #[error("Registry error: {0}")]
    Registry(String),

    #[error("Index error: {0}")]
    Index(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("gRPC error: {0}")]
    Grpc(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, KeelError>;
