//! Common error types for AxiomVault.

use thiserror::Error;

/// Top-level error type for AxiomVault operations.
#[derive(Debug, Error)]
pub enum Error {
    /// Cryptographic operation failed.
    #[error("Cryptographic error: {0}")]
    Crypto(String),

    /// Vault operation failed.
    #[error("Vault error: {0}")]
    Vault(String),

    /// Storage operation failed.
    #[error("Storage error: {0}")]
    Storage(String),

    /// I/O operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Serialization or deserialization failed.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Invalid input provided.
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    /// Operation not permitted.
    #[error("Not permitted: {0}")]
    NotPermitted(String),

    /// Resource not found.
    #[error("Not found: {0}")]
    NotFound(String),

    /// Resource already exists.
    #[error("Already exists: {0}")]
    AlreadyExists(String),

    /// Conflict detected.
    #[error("Conflict: {0}")]
    Conflict(String),
}

/// Result type alias using the common Error.
pub type Result<T> = std::result::Result<T, Error>;
