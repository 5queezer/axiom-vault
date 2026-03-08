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
