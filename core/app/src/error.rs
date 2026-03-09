//! Application-level error taxonomy.
//!
//! Maps internal errors to user-facing categories that UI shells can
//! present without leaking implementation details.

use axiomvault_common::Error as CommonError;

/// Application-level error categories.
///
/// Each variant maps to a user-facing error condition that UI shells
/// can handle with appropriate messaging and recovery actions.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Vault does not exist at the specified location.
    #[error("Vault not found: {0}")]
    VaultNotFound(String),

    /// Vault already exists at the specified location.
    #[error("Vault already exists: {0}")]
    VaultAlreadyExists(String),

    /// Password is incorrect.
    #[error("Invalid password")]
    InvalidPassword,

    /// Recovery key is incorrect.
    #[error("Invalid recovery key")]
    InvalidRecoveryKey,

    /// No vault is currently open.
    #[error("No vault is open")]
    NoOpenVault,

    /// Session is locked and requires unlock.
    #[error("Vault is locked")]
    VaultLocked,

    /// File or directory not found within the vault.
    #[error("Path not found: {0}")]
    PathNotFound(String),

    /// File or directory already exists.
    #[error("Path already exists: {0}")]
    PathAlreadyExists(String),

    /// Invalid path or input.
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    /// Storage provider error (network, I/O, auth).
    #[error("Storage error: {0}")]
    Storage(String),

    /// Sync conflict detected.
    #[error("Sync conflict: {0}")]
    SyncConflict(String),

    /// Cryptographic operation failed.
    #[error("Encryption error: {0}")]
    Crypto(String),

    /// Internal error that should not happen.
    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<CommonError> for AppError {
    fn from(err: CommonError) -> Self {
        match err {
            CommonError::NotFound(msg) if msg.contains("Vault") || msg.contains("vault") => {
                AppError::VaultNotFound(msg)
            }
            CommonError::NotFound(msg) => AppError::PathNotFound(msg),
            CommonError::AlreadyExists(msg) => AppError::PathAlreadyExists(msg),
            CommonError::NotPermitted(msg) if msg.contains("password") => AppError::InvalidPassword,
            CommonError::NotPermitted(msg) if msg.contains("recovery") => {
                AppError::InvalidRecoveryKey
            }
            CommonError::NotPermitted(msg) if msg.contains("locked") => AppError::VaultLocked,
            CommonError::NotPermitted(msg) => AppError::InvalidInput(msg),
            CommonError::InvalidInput(msg) => AppError::InvalidInput(msg),
            CommonError::Crypto(msg) => AppError::Crypto(msg),
            CommonError::Storage(msg) => AppError::Storage(msg),
            CommonError::Network(msg) => AppError::Storage(msg),
            CommonError::Authentication(msg) => AppError::Storage(msg),
            CommonError::Io(err) => AppError::Storage(err.to_string()),
            CommonError::Serialization(msg) => AppError::Internal(msg),
            CommonError::Vault(msg) => AppError::Internal(msg),
            CommonError::Conflict(msg) => AppError::SyncConflict(msg),
        }
    }
}

pub type AppResult<T> = std::result::Result<T, AppError>;
