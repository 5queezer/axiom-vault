//! FFI bindings for AxiomVault core.
//!
//! Provides C-ABI compatible functions for mobile platforms (iOS/Android).
//! All operations delegate to `AppService`, the shared application facade.
//!
//! # Bridge strategy
//!
//! This crate uses **cbindgen** to generate C headers consumed by Swift via
//! an Objective-C bridging header. This was chosen over uniffi because:
//!
//! - The existing XCFramework build pipeline is built around cbindgen
//! - Full ABI control matters for a security-sensitive product
//! - The Swift wrapper layer (`VaultCore.swift`) is thin and maintainable
//! - cbindgen avoids the build-time cost of uniffi-bindgen codegen
//!
//! # Event subscription
//!
//! Swift clients can subscribe to `AppEvent` notifications via
//! `axiom_vault_subscribe_events`, which accepts a C function pointer.
//! Events are delivered as JSON strings on a background thread.

#![allow(clippy::missing_safety_doc)]

pub mod error;
pub mod runtime;
pub mod types;
pub mod vault_ops;

use std::ffi::{c_char, c_int, CStr, CString};
use std::ptr;

use crate::error::FFIError;
use crate::runtime::get_runtime;
use crate::types::{FFIEventCallback, FFIVaultHandle, FFIVaultInfo};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract a `&str` from a C string pointer, setting the FFI error on failure.
unsafe fn str_from_ptr<'a>(ptr: *const c_char, name: &str) -> Option<&'a str> {
    if ptr.is_null() {
        error::set_last_error(FFIError::NullPointer(format!("{} is null", name)));
        return None;
    }
    match CStr::from_ptr(ptr).to_str() {
        Ok(s) => Some(s),
        Err(_) => {
            error::set_last_error(FFIError::InvalidUtf8(name.to_string()));
            None
        }
    }
}

/// Run an async operation on the global runtime, mapping errors to FFI.
fn block_on<F, T>(f: F) -> Result<T, ()>
where
    F: std::future::Future<Output = Result<T, FFIError>>,
{
    let runtime = match get_runtime() {
        Ok(rt) => rt,
        Err(e) => {
            error::set_last_error(FFIError::RuntimeError(e.to_string()));
            return Err(());
        }
    };
    match runtime.block_on(f) {
        Ok(v) => Ok(v),
        Err(e) => {
            error::set_last_error(e);
            Err(())
        }
    }
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the FFI layer. Must be called before any other FFI functions.
///
/// # Safety
/// This function is safe to call from foreign code.
#[no_mangle]
pub extern "C" fn axiom_init() -> c_int {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();

    match get_runtime() {
        Ok(_) => {
            tracing::info!("AxiomVault FFI initialized");
            0
        }
        Err(e) => {
            tracing::error!("Failed to initialize runtime: {}", e);
            -1
        }
    }
}

/// Get the version of the AxiomVault library.
///
/// # Safety
/// Returns a pointer to a static string. Do not free.
#[no_mangle]
pub extern "C" fn axiom_version() -> *const c_char {
    static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");
    VERSION.as_ptr() as *const c_char
}

// ---------------------------------------------------------------------------
// Vault lifecycle
// ---------------------------------------------------------------------------

/// Create a new vault at the specified path with the given password.
///
/// # Safety
/// - `path` must be a valid null-terminated UTF-8 string
/// - `password` must be a valid null-terminated UTF-8 string
/// - Returns a handle that must be freed with `axiom_vault_close`
/// - On error, returns null and sets error message retrievable via `axiom_last_error`
#[no_mangle]
pub unsafe extern "C" fn axiom_vault_create(
    path: *const c_char,
    password: *const c_char,
) -> *mut FFIVaultHandle {
    let path_str = match str_from_ptr(path, "path") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let password_str = match str_from_ptr(password, "password") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };

    match block_on(vault_ops::create_vault(path_str, password_str)) {
        Ok(handle) => Box::into_raw(Box::new(handle)),
        Err(()) => ptr::null_mut(),
    }
}

/// Open an existing vault at the specified path with the given password.
///
/// # Safety
/// - `path` must be a valid null-terminated UTF-8 string
/// - `password` must be a valid null-terminated UTF-8 string
/// - Returns a handle that must be freed with `axiom_vault_close`
#[no_mangle]
pub unsafe extern "C" fn axiom_vault_open(
    path: *const c_char,
    password: *const c_char,
) -> *mut FFIVaultHandle {
    let path_str = match str_from_ptr(path, "path") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let password_str = match str_from_ptr(password, "password") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };

    match block_on(vault_ops::open_vault(path_str, password_str)) {
        Ok(handle) => Box::into_raw(Box::new(handle)),
        Err(()) => ptr::null_mut(),
    }
}

/// Close a vault and free its resources.
///
/// # Safety
/// - `handle` must be a valid handle returned by `axiom_vault_create` or `axiom_vault_open`
/// - After this call, the handle is invalid and must not be used
#[no_mangle]
pub unsafe extern "C" fn axiom_vault_close(handle: *mut FFIVaultHandle) -> c_int {
    if handle.is_null() {
        error::set_last_error(FFIError::NullPointer("handle is null".into()));
        return -1;
    }

    let handle = Box::from_raw(handle);

    // Abort any active event subscription task.
    if let Ok(mut guard) = handle.event_task.lock() {
        if let Some(task) = guard.take() {
            task.abort();
        }
    }

    let runtime = match get_runtime() {
        Ok(rt) => rt,
        Err(e) => {
            error::set_last_error(FFIError::RuntimeError(e.to_string()));
            return -1;
        }
    };

    // Close the vault through AppService so the index is wiped.
    if let Err(e) = runtime.block_on(handle.service.close_vault()) {
        // Log but don't fail — the handle is being freed regardless.
        tracing::warn!("Error closing vault: {}", e);
    }
    0
}

// ---------------------------------------------------------------------------
// Vault info
// ---------------------------------------------------------------------------

/// Get information about an open vault.
///
/// # Safety
/// - `handle` must be a valid vault handle
/// - Returns a pointer to `FFIVaultInfo` that must be freed with `axiom_vault_info_free`
#[no_mangle]
pub unsafe extern "C" fn axiom_vault_info(handle: *const FFIVaultHandle) -> *mut FFIVaultInfo {
    if handle.is_null() {
        error::set_last_error(FFIError::NullPointer("handle is null".into()));
        return ptr::null_mut();
    }

    match vault_ops::get_vault_info(&*handle) {
        Ok(info) => Box::into_raw(Box::new(info)),
        Err(e) => {
            error::set_last_error(e);
            ptr::null_mut()
        }
    }
}

/// Free vault info structure.
///
/// # Safety
/// - `info` must be a valid pointer returned by `axiom_vault_info`
#[no_mangle]
pub unsafe extern "C" fn axiom_vault_info_free(info: *mut FFIVaultInfo) {
    if !info.is_null() {
        let info = Box::from_raw(info);
        if !info.vault_id.is_null() {
            let _ = CString::from_raw(info.vault_id as *mut c_char);
        }
        if !info.root_path.is_null() {
            let _ = CString::from_raw(info.root_path as *mut c_char);
        }
    }
}

// ---------------------------------------------------------------------------
// File and directory operations
// ---------------------------------------------------------------------------

/// List files in the vault at the specified path.
///
/// # Safety
/// - `handle` must be a valid vault handle
/// - `path` must be a valid null-terminated UTF-8 string (use "/" for root)
/// - Returns a JSON string containing the file list
/// - Returned string must be freed with `axiom_string_free`
#[no_mangle]
pub unsafe extern "C" fn axiom_vault_list(
    handle: *const FFIVaultHandle,
    path: *const c_char,
) -> *mut c_char {
    if handle.is_null() {
        error::set_last_error(FFIError::NullPointer("handle is null".into()));
        return ptr::null_mut();
    }
    let path_str = match str_from_ptr(path, "path") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };

    match block_on(vault_ops::list_vault(&*handle, path_str)) {
        Ok(json) => CString::new(json)
            .map(|s| s.into_raw())
            .unwrap_or_else(|_| {
                error::set_last_error(FFIError::StringConversionError);
                ptr::null_mut()
            }),
        Err(()) => ptr::null_mut(),
    }
}

/// Add a file to the vault.
///
/// # Safety
/// - `handle` must be a valid vault handle
/// - `local_path` must be a valid null-terminated UTF-8 string (path to local file)
/// - `vault_path` must be a valid null-terminated UTF-8 string (path in vault)
#[no_mangle]
pub unsafe extern "C" fn axiom_vault_add_file(
    handle: *const FFIVaultHandle,
    local_path: *const c_char,
    vault_path: *const c_char,
) -> c_int {
    if handle.is_null() {
        error::set_last_error(FFIError::NullPointer("handle is null".into()));
        return -1;
    }
    let local_str = match str_from_ptr(local_path, "local_path") {
        Some(s) => s,
        None => return -1,
    };
    let vault_str = match str_from_ptr(vault_path, "vault_path") {
        Some(s) => s,
        None => return -1,
    };

    match block_on(vault_ops::add_file(&*handle, local_str, vault_str)) {
        Ok(()) => 0,
        Err(()) => -1,
    }
}

/// Extract a file from the vault.
///
/// # Safety
/// - `handle` must be a valid vault handle
/// - `vault_path` must be a valid null-terminated UTF-8 string (path in vault)
/// - `local_path` must be a valid null-terminated UTF-8 string (path to save file)
#[no_mangle]
pub unsafe extern "C" fn axiom_vault_extract_file(
    handle: *const FFIVaultHandle,
    vault_path: *const c_char,
    local_path: *const c_char,
) -> c_int {
    if handle.is_null() {
        error::set_last_error(FFIError::NullPointer("handle is null".into()));
        return -1;
    }
    let vault_str = match str_from_ptr(vault_path, "vault_path") {
        Some(s) => s,
        None => return -1,
    };
    let local_str = match str_from_ptr(local_path, "local_path") {
        Some(s) => s,
        None => return -1,
    };

    match block_on(vault_ops::extract_file(&*handle, vault_str, local_str)) {
        Ok(()) => 0,
        Err(()) => -1,
    }
}

/// Create a directory in the vault.
///
/// # Safety
/// - `handle` must be a valid vault handle
/// - `vault_path` must be a valid null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn axiom_vault_mkdir(
    handle: *const FFIVaultHandle,
    vault_path: *const c_char,
) -> c_int {
    if handle.is_null() {
        error::set_last_error(FFIError::NullPointer("handle is null".into()));
        return -1;
    }
    let vault_str = match str_from_ptr(vault_path, "vault_path") {
        Some(s) => s,
        None => return -1,
    };

    match block_on(vault_ops::create_directory(&*handle, vault_str)) {
        Ok(()) => 0,
        Err(()) => -1,
    }
}

/// Remove a file or directory from the vault.
///
/// # Safety
/// - `handle` must be a valid vault handle
/// - `vault_path` must be a valid null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn axiom_vault_remove(
    handle: *const FFIVaultHandle,
    vault_path: *const c_char,
) -> c_int {
    if handle.is_null() {
        error::set_last_error(FFIError::NullPointer("handle is null".into()));
        return -1;
    }
    let vault_str = match str_from_ptr(vault_path, "vault_path") {
        Some(s) => s,
        None => return -1,
    };

    match block_on(vault_ops::remove_entry(&*handle, vault_str)) {
        Ok(()) => 0,
        Err(()) => -1,
    }
}

// ---------------------------------------------------------------------------
// Password and recovery
// ---------------------------------------------------------------------------

/// Change the vault password.
///
/// # Safety
/// - `handle` must be a valid vault handle
/// - `old_password` and `new_password` must be valid null-terminated UTF-8 strings
#[no_mangle]
pub unsafe extern "C" fn axiom_vault_change_password(
    handle: *const FFIVaultHandle,
    old_password: *const c_char,
    new_password: *const c_char,
) -> c_int {
    if handle.is_null() {
        error::set_last_error(FFIError::NullPointer("handle is null".into()));
        return -1;
    }
    let old_str = match str_from_ptr(old_password, "old_password") {
        Some(s) => s,
        None => return -1,
    };
    let new_str = match str_from_ptr(new_password, "new_password") {
        Some(s) => s,
        None => return -1,
    };

    match block_on(vault_ops::change_password(&*handle, old_str, new_str)) {
        Ok(()) => 0,
        Err(()) => -1,
    }
}

/// Get the recovery words from a newly created vault.
///
/// Only returns words if the handle was obtained via `axiom_vault_create`.
/// Returns null if the vault was opened (not created) or words were already
/// consumed by a previous call. **Words are cleared after retrieval** — this
/// function returns them exactly once.
///
/// # Safety
/// - `handle` must be a valid vault handle
/// - Returned string must be freed with `axiom_string_free`
#[no_mangle]
pub unsafe extern "C" fn axiom_vault_get_recovery_words(
    handle: *const FFIVaultHandle,
) -> *mut c_char {
    if handle.is_null() {
        error::set_last_error(FFIError::NullPointer("handle is null".into()));
        return ptr::null_mut();
    }

    let words = (*handle)
        .recovery_words
        .lock()
        .ok()
        .and_then(|mut guard| guard.take());

    match words {
        Some(w) => CString::new(w).map(|s| s.into_raw()).unwrap_or_else(|_| {
            error::set_last_error(FFIError::StringConversionError);
            ptr::null_mut()
        }),
        None => ptr::null_mut(),
    }
}

/// Show recovery key for an open vault (requires active session).
///
/// # Safety
/// - `handle` must be a valid vault handle
/// - Returned string must be freed with `axiom_string_free`
#[no_mangle]
pub unsafe extern "C" fn axiom_vault_show_recovery_key(
    handle: *const FFIVaultHandle,
) -> *mut c_char {
    if handle.is_null() {
        error::set_last_error(FFIError::NullPointer("handle is null".into()));
        return ptr::null_mut();
    }

    match block_on(vault_ops::show_recovery_key(&*handle)) {
        Ok(words) => CString::new(words)
            .map(|s| s.into_raw())
            .unwrap_or_else(|_| {
                error::set_last_error(FFIError::StringConversionError);
                ptr::null_mut()
            }),
        Err(()) => ptr::null_mut(),
    }
}

/// Reset the vault password using recovery key words.
///
/// # Safety
/// - `path` must be a valid null-terminated UTF-8 string
/// - `recovery_words` must be a valid null-terminated UTF-8 string (24 space-separated words)
/// - `new_password` must be a valid null-terminated UTF-8 string
/// - Returns a vault handle on success, null on failure
#[no_mangle]
pub unsafe extern "C" fn axiom_vault_reset_password(
    path: *const c_char,
    recovery_words: *const c_char,
    new_password: *const c_char,
) -> *mut FFIVaultHandle {
    let path_str = match str_from_ptr(path, "path") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let words_str = match str_from_ptr(recovery_words, "recovery_words") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let password_str = match str_from_ptr(new_password, "new_password") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };

    match block_on(vault_ops::reset_password(path_str, words_str, password_str)) {
        Ok(handle) => Box::into_raw(Box::new(handle)),
        Err(()) => ptr::null_mut(),
    }
}

// ---------------------------------------------------------------------------
// Health check and migration
// ---------------------------------------------------------------------------

/// Check if a vault at the given path needs migration.
///
/// # Safety
/// - `path` must be a valid null-terminated UTF-8 string pointing to a vault directory
///
/// # Returns
/// - 0: up to date
/// - 1: needs migration
/// - -1: incompatible or error (check `axiom_last_error`)
#[no_mangle]
pub unsafe extern "C" fn axiom_vault_check_migration(path: *const c_char) -> c_int {
    let path_str = match str_from_ptr(path, "path") {
        Some(s) => s,
        None => return -1,
    };

    match vault_ops::check_migration(path_str) {
        Ok(status) => status,
        Err(e) => {
            error::set_last_error(e);
            -1
        }
    }
}

/// Run migrations on a vault at the given path.
///
/// # Safety
/// - `path` must be a valid null-terminated UTF-8 string pointing to a vault directory
/// - `password` must be a valid null-terminated UTF-8 string
///
/// # Returns
/// - 0 on success
/// - -1 on error (check `axiom_last_error`)
#[no_mangle]
pub unsafe extern "C" fn axiom_vault_migrate(
    path: *const c_char,
    password: *const c_char,
) -> c_int {
    let path_str = match str_from_ptr(path, "path") {
        Some(s) => s,
        None => return -1,
    };
    let password_str = match str_from_ptr(password, "password") {
        Some(s) => s,
        None => return -1,
    };

    match vault_ops::run_migration(path_str, password_str) {
        Ok(()) => 0,
        Err(e) => {
            error::set_last_error(e);
            -1
        }
    }
}

/// Run a vault health check and return results as JSON.
///
/// # Safety
/// - `path` must be a valid null-terminated UTF-8 string
/// - `password` may be null for a shallow (structure-only) check
/// - Returned string must be freed with `axiom_string_free`
#[no_mangle]
pub unsafe extern "C" fn axiom_vault_health_check(
    path: *const c_char,
    password: *const c_char,
) -> *mut c_char {
    let path_str = match str_from_ptr(path, "path") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };

    let password_opt = if password.is_null() {
        None
    } else {
        match CStr::from_ptr(password).to_str() {
            Ok(s) => Some(s),
            Err(_) => {
                error::set_last_error(FFIError::InvalidUtf8("password".into()));
                return ptr::null_mut();
            }
        }
    };

    match block_on(vault_ops::health_check(path_str, password_opt)) {
        Ok(json) => CString::new(json)
            .map(|s| s.into_raw())
            .unwrap_or_else(|_| {
                error::set_last_error(FFIError::StringConversionError);
                ptr::null_mut()
            }),
        Err(()) => ptr::null_mut(),
    }
}

// ---------------------------------------------------------------------------
// Event subscription
// ---------------------------------------------------------------------------

/// Subscribe to vault events. The callback receives JSON-encoded `AppEvent`
/// strings on a background thread.
///
/// Only one subscription is active per handle. Calling again **aborts** the
/// previous forwarding task before starting a new one. Pass a null callback
/// to unsubscribe without starting a new subscription.
///
/// # Safety
/// - `handle` must be a valid vault handle
/// - `callback` must be a valid function pointer or null
/// - The callback may be invoked from any thread
#[no_mangle]
pub unsafe extern "C" fn axiom_vault_subscribe_events(
    handle: *const FFIVaultHandle,
    callback: Option<FFIEventCallback>,
) -> c_int {
    if handle.is_null() {
        error::set_last_error(FFIError::NullPointer("handle is null".into()));
        return -1;
    }

    let handle = &*handle;

    // Abort any existing subscription task.
    if let Ok(mut guard) = handle.event_task.lock() {
        if let Some(task) = guard.take() {
            task.abort();
        }
    }

    // If no callback, we just unsubscribed — done.
    let callback = match callback {
        Some(cb) => cb,
        None => return 0,
    };

    let mut rx = handle.service.subscribe();

    let runtime = match get_runtime() {
        Ok(rt) => rt,
        Err(e) => {
            error::set_last_error(FFIError::RuntimeError(e.to_string()));
            return -1;
        }
    };

    let task = runtime.spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Ok(json) = serde_json::to_string(&event) {
                        if let Ok(cstr) = CString::new(json) {
                            callback(cstr.as_ptr());
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
    });

    // Store the task handle so it can be aborted on re-subscribe or close.
    if let Ok(mut guard) = handle.event_task.lock() {
        *guard = Some(task);
    }

    0
}

// ---------------------------------------------------------------------------
// Error and string management
// ---------------------------------------------------------------------------

/// Get the last error message.
///
/// # Safety
/// - Returns a pointer to an error string
/// - Returned string must be freed with `axiom_string_free`
/// - Returns null if no error occurred
#[no_mangle]
pub extern "C" fn axiom_last_error() -> *mut c_char {
    error::take_last_error()
        .map(|e| {
            CString::new(e.to_string())
                .map(|s| s.into_raw())
                .unwrap_or(ptr::null_mut())
        })
        .unwrap_or(ptr::null_mut())
}

/// Free a string returned by an FFI function.
///
/// # Safety
/// - `s` must be a valid pointer returned by an axiom FFI function
/// - After this call, the pointer is invalid
#[no_mangle]
pub unsafe extern "C" fn axiom_string_free(s: *mut c_char) {
    if !s.is_null() {
        let _ = CString::from_raw(s);
    }
}
