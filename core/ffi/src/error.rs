//! FFI error handling
//!
//! Thread-local error storage for FFI functions.

use std::cell::RefCell;
use std::fmt;

use axiomvault_app::AppError;

/// FFI-specific errors.
#[derive(Debug, Clone)]
pub enum FFIError {
    /// Null pointer passed to FFI function.
    NullPointer(String),
    /// Invalid UTF-8 in string parameter.
    InvalidUtf8(String),
    /// Runtime initialization error.
    RuntimeError(String),
    /// Vault operation error.
    VaultError(String),
    /// Storage operation error.
    StorageError(String),
    /// Crypto operation error.
    CryptoError(String),
    /// String conversion error.
    StringConversionError,
    /// IO error.
    IOError(String),
}

impl fmt::Display for FFIError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FFIError::NullPointer(msg) => write!(f, "Null pointer: {}", msg),
            FFIError::InvalidUtf8(param) => write!(f, "Invalid UTF-8 in parameter: {}", param),
            FFIError::RuntimeError(msg) => write!(f, "Runtime error: {}", msg),
            FFIError::VaultError(msg) => write!(f, "Vault error: {}", msg),
            FFIError::StorageError(msg) => write!(f, "Storage error: {}", msg),
            FFIError::CryptoError(msg) => write!(f, "Crypto error: {}", msg),
            FFIError::StringConversionError => write!(f, "String conversion error"),
            FFIError::IOError(msg) => write!(f, "IO error: {}", msg),
        }
    }
}

impl std::error::Error for FFIError {}

impl From<AppError> for FFIError {
    fn from(err: AppError) -> Self {
        match err {
            AppError::VaultNotFound(msg) => {
                FFIError::VaultError(format!("Vault not found: {}", msg))
            }
            AppError::VaultAlreadyExists(msg) => {
                FFIError::VaultError(format!("Vault already exists: {}", msg))
            }
            AppError::InvalidPassword => FFIError::CryptoError("Invalid password".to_string()),
            AppError::InvalidRecoveryKey => {
                FFIError::CryptoError("Invalid recovery key".to_string())
            }
            AppError::NoOpenVault => FFIError::VaultError("No vault is open".to_string()),
            AppError::VaultLocked => FFIError::VaultError("Vault is locked".to_string()),
            AppError::PathNotFound(msg) => FFIError::VaultError(format!("Path not found: {}", msg)),
            AppError::PathAlreadyExists(msg) => {
                FFIError::VaultError(format!("Path already exists: {}", msg))
            }
            AppError::InvalidInput(msg) => FFIError::VaultError(format!("Invalid input: {}", msg)),
            AppError::Storage(msg) => FFIError::StorageError(msg),
            AppError::SyncConflict(msg) => FFIError::VaultError(format!("Sync conflict: {}", msg)),
            AppError::Crypto(msg) => FFIError::CryptoError(msg),
            AppError::Internal(msg) => FFIError::VaultError(format!("Internal error: {}", msg)),
        }
    }
}

/// Result type for FFI operations.
pub type FFIResult<T> = Result<T, FFIError>;

thread_local! {
    static LAST_ERROR: RefCell<Option<FFIError>> = const { RefCell::new(None) };
}

/// Set the last error for the current thread.
pub fn set_last_error(error: FFIError) {
    tracing::error!("FFI error: {}", error);
    LAST_ERROR.with(|e| {
        *e.borrow_mut() = Some(error);
    });
}

/// Take the last error from the current thread.
pub fn take_last_error() -> Option<FFIError> {
    LAST_ERROR.with(|e| e.borrow_mut().take())
}
