// Unified Error type for the root keel binary

use thiserror::Error;

#[derive(Error, Debug)]
pub enum KeelError {
    #[error("{0}")]
    Store(#[from] keel_store::error::StoreError),

    #[error("gRPC error: {0}")]
    Grpc(String),

    #[error("Configuration error: {0}")]
    Config(String),
}

pub type Result<T> = std::result::Result<T, KeelError>;
