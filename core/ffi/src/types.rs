//! FFI-safe types
//!
//! Types that can cross the FFI boundary safely.

use std::ffi::{c_char, c_int, c_longlong};

use axiomvault_app::AppService;

/// Opaque handle to the application service.
///
/// Wraps `AppService` — the single entry point for all vault operations.
/// C code should treat this as an opaque pointer.
pub struct FFIVaultHandle {
    pub(crate) service: AppService,
    /// Vault storage path on disk (for health check / migration).
    pub(crate) path: String,
    /// Recovery words from vault creation (only set on create, not on open).
    pub(crate) recovery_words: Option<String>,
}

/// Vault information structure (C-safe).
#[repr(C)]
pub struct FFIVaultInfo {
    /// Vault ID (null-terminated string, caller must free).
    pub vault_id: *const c_char,
    /// Root path of the vault.
    pub root_path: *const c_char,
    /// Number of files in the vault.
    pub file_count: c_int,
    /// Total size in bytes.
    pub total_size: c_longlong,
    /// Vault version.
    pub version: c_int,
}

/// Callback for receiving events as JSON strings.
///
/// The callback is invoked on a background thread. The `json` pointer is
/// only valid for the duration of the call — copy the string if you need
/// to retain it.
pub type FFIEventCallback = extern "C" fn(json: *const c_char);
