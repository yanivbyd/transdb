use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Error types for TranDB operations
#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TranDbError {
    #[error("Key not found: {0}")]
    KeyNotFound(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Server error: {0}")]
    ServerError(String),
}

/// Result type for TranDB operations
pub type Result<T> = std::result::Result<T, TranDbError>;
