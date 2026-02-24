use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_KEY_SIZE: usize = 1_024;
pub const MAX_VALUE_SIZE: usize = 4_194_304;

/// Error types for TranDB operations
#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TranDbError {
    #[error("Key not found: {0}")]
    KeyNotFound(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("HTTP {0}: {1}")]
    HttpError(u16, String),

    #[error("Key exceeds maximum size of {0} bytes")]
    KeyTooLarge(usize),

    #[error("Value exceeds maximum size of {0} bytes")]
    ValueTooLarge(usize),

    #[error("Server response missing ETag header")]
    MissingETag,
}

/// JSON error envelope returned by the server for all error responses
#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

/// Result type for TranDB operations
pub type Result<T> = std::result::Result<T, TranDbError>;
