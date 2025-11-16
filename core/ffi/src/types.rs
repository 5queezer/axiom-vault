//! FFI-safe types
//!
//! Types that can cross the FFI boundary safely.

use std::ffi::{c_char, c_int, c_longlong};
use std::sync::Arc;
use tokio::sync::RwLock;

use axiomvault_vault::VaultSession;

/// Internal vault handle data (opaque to C code)
pub struct VaultHandleData {
    /// Internal session
    pub(crate) session: Arc<RwLock<VaultSession>>,
    /// Vault path on disk
    pub(crate) path: String,
}

/// Opaque handle to an open vault session
///
/// This is an opaque pointer to internal Rust data.
/// C code should treat this as an opaque pointer.
pub type FFIVaultHandle = VaultHandleData;

/// Vault information structure
#[repr(C)]
pub struct FFIVaultInfo {
    /// Vault ID (null-terminated string, caller must free)
    pub vault_id: *const c_char,
    /// Root path of the vault
    pub root_path: *const c_char,
    /// Number of files in the vault
    pub file_count: c_int,
    /// Total size in bytes
    pub total_size: c_longlong,
    /// Vault version
    pub version: c_int,
}

/// File entry information
#[repr(C)]
pub struct FFIFileEntry {
    /// File name (null-terminated string)
    pub name: *const c_char,
    /// File path in vault
    pub path: *const c_char,
    /// Is directory
    pub is_directory: c_int,
    /// File size in bytes (0 for directories)
    pub size: c_longlong,
    /// Last modified timestamp (Unix timestamp)
    pub modified: c_longlong,
}

/// Sync status enumeration
#[repr(C)]
pub enum FFISyncStatus {
    Synced = 0,
    LocalModified = 1,
    RemoteModified = 2,
    Conflicted = 3,
    Syncing = 4,
    Failed = 5,
}

/// Conflict resolution strategy
#[repr(C)]
pub enum FFIConflictStrategy {
    KeepBoth = 0,
    PreferLocal = 1,
    PreferRemote = 2,
}
