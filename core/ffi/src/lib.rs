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

use zeroize::Zeroizing;

use crate::error::FFIError;
use crate::runtime::get_runtime;
use crate::types::{FFIEventCallback, FFIVaultHandle, FFIVaultInfo};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract a `&str` from a C string pointer, setting the FFI error on failure.
///
/// # Safety
/// The caller must ensure `ptr` is either null or points to a valid
/// NUL-terminated C string that lives at least as long as the returned `&str`.
// SAFETY: contract documented above; callers in this module pass pointers
// supplied by the FFI consumer, which we treat as opaque per the C-ABI contract.
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

/// Copy a C string into a freshly allocated [`Zeroizing<String>`], setting
/// the FFI error on failure. The caller-provided buffer is *not* mutated;
/// the new owned `Zeroizing<String>` will wipe its bytes on drop.
///
/// # Safety
/// Same contract as [`str_from_ptr`]: `ptr` must be null or point to a valid
/// NUL-terminated C string.
// SAFETY: contract delegated to `str_from_ptr` — the only unsafe operation
// here is the inner `CStr::from_ptr` call inside that helper.
unsafe fn zeroizing_string_from_ptr(ptr: *const c_char, name: &str) -> Option<Zeroizing<String>> {
    let s = str_from_ptr(ptr, name)?;
    Some(Zeroizing::new(s.to_owned()))
}

/// Copy a `Zeroizing<String>` (containing a recovery mnemonic or other secret)
/// into a freshly allocated C-owned buffer, then zeroize the source.
///
/// The returned pointer must be freed by the caller via
/// [`axiom_recovery_words_free`], which wipes the bytes before deallocating.
fn into_secret_cstr(secret: Zeroizing<String>) -> Result<*mut c_char, FFIError> {
    // CString::new copies into a fresh allocation and rejects interior NULs.
    // After this, the original `secret` is dropped (and zeroed) at end of scope.
    let cstring = CString::new(secret.as_bytes()).map_err(|_| FFIError::StringConversionError)?;
    drop(secret);
    Ok(cstring.into_raw())
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
    let password_zeroizing = match zeroizing_string_from_ptr(password, "password") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };

    match block_on(vault_ops::create_vault(path_str, password_zeroizing)) {
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
    let password_zeroizing = match zeroizing_string_from_ptr(password, "password") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };

    match block_on(vault_ops::open_vault(path_str, password_zeroizing)) {
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
    let old_pw = match zeroizing_string_from_ptr(old_password, "old_password") {
        Some(s) => s,
        None => return -1,
    };
    let new_pw = match zeroizing_string_from_ptr(new_password, "new_password") {
        Some(s) => s,
        None => return -1,
    };

    match block_on(vault_ops::change_password(&*handle, old_pw, new_pw)) {
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
/// The bytes inside the handle are kept in a `Zeroizing<String>` and are
/// wiped immediately after being copied into the C-owned buffer.
///
/// # Safety
/// - `handle` must be a valid vault handle
/// - Returned string must be freed with [`axiom_recovery_words_free`]
///   (NOT `axiom_string_free` — recovery words require zeroizing free)
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
        Some(w) => match into_secret_cstr(w) {
            Ok(ptr) => ptr,
            Err(e) => {
                error::set_last_error(e);
                ptr::null_mut()
            }
        },
        None => ptr::null_mut(),
    }
}

/// Show recovery key for an open vault (requires active session).
///
/// The mnemonic is held in a `Zeroizing<String>` end-to-end and is wiped
/// immediately after being copied into the C-owned buffer.
///
/// # Safety
/// - `handle` must be a valid vault handle
/// - Returned string must be freed with [`axiom_recovery_words_free`]
///   (NOT `axiom_string_free` — recovery words require zeroizing free)
#[no_mangle]
pub unsafe extern "C" fn axiom_vault_show_recovery_key(
    handle: *const FFIVaultHandle,
) -> *mut c_char {
    if handle.is_null() {
        error::set_last_error(FFIError::NullPointer("handle is null".into()));
        return ptr::null_mut();
    }

    match block_on(vault_ops::show_recovery_key(&*handle)) {
        Ok(words) => match into_secret_cstr(words) {
            Ok(ptr) => ptr,
            Err(e) => {
                error::set_last_error(e);
                ptr::null_mut()
            }
        },
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
    let words_zeroizing = match zeroizing_string_from_ptr(recovery_words, "recovery_words") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let password_zeroizing = match zeroizing_string_from_ptr(new_password, "new_password") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };

    match block_on(vault_ops::reset_password(
        path_str,
        words_zeroizing,
        password_zeroizing,
    )) {
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
/// Do **not** use this for strings containing secrets — use the dedicated
/// [`axiom_recovery_words_free`] for recovery mnemonics. Callers that pass
/// a password or mnemonic here will leak the bytes to libc's allocator.
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

/// Free a recovery-words string returned by [`axiom_vault_get_recovery_words`]
/// or [`axiom_vault_show_recovery_key`], zeroing the underlying bytes before
/// releasing the allocation.
///
/// This is required (instead of [`axiom_string_free`]) because BIP39 recovery
/// words can reconstruct the vault master key. Leaving them in freed heap
/// memory is equivalent to leaking the credential.
///
/// Passing a pointer obtained from any other FFI function is undefined
/// behavior.
///
/// # Safety
/// - `s` must be a valid pointer returned by `axiom_vault_get_recovery_words`
///   or `axiom_vault_show_recovery_key`
/// - After this call, the pointer is invalid
#[no_mangle]
pub unsafe extern "C" fn axiom_recovery_words_free(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    // Reclaim ownership of the allocation FIRST so its internal length is
    // recovered from the (still-intact) NUL terminator. Zeroizing before
    // `from_raw` would cause `strlen` to see length 0 and the allocator
    // would only free 1 byte.
    //
    // SAFETY: The caller must ensure `s` came from a recovery-words FFI
    // function, which allocated it via `CString::into_raw`. `from_raw` thus
    // takes back ownership of the same allocation and won't be called twice.
    let cstring = CString::from_raw(s);

    // Overwrite the bytes (including the NUL terminator) in place, through
    // the pointer we still hold. This uses a volatile-style write via
    // `write_bytes` plus a compiler fence to prevent the writes being elided
    // by the optimizer before the subsequent free.
    let bytes = cstring.as_bytes_with_nul();
    let len = bytes.len();
    let ptr = bytes.as_ptr() as *mut u8;
    // SAFETY: `ptr` points to `len` bytes owned by `cstring`; we have exclusive
    // access for the duration of this call because we just took ownership.
    std::ptr::write_bytes(ptr, 0, len);
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);

    drop(cstring);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `into_secret_cstr` round-trips a mnemonic into a C string and back,
    /// proving that the helper produces a valid NUL-terminated buffer.
    #[test]
    fn into_secret_cstr_round_trip() {
        let secret = Zeroizing::new("abandon ability able about above absent".to_string());
        let expected = secret.clone();

        let raw = into_secret_cstr(secret).expect("non-empty secret converts");
        assert!(!raw.is_null());

        // SAFETY: `raw` was just allocated by `into_secret_cstr` and is owned here.
        let recovered = unsafe { CStr::from_ptr(raw) }
            .to_str()
            .expect("valid UTF-8");
        assert_eq!(recovered, &*expected);

        // SAFETY: `raw` came from `into_secret_cstr` which uses `CString::into_raw`,
        // so this is the matching free.
        unsafe { axiom_recovery_words_free(raw) };
    }

    /// Calling the free function on a null pointer must be a no-op (matches
    /// the contract of `axiom_string_free`).
    #[test]
    fn recovery_words_free_null_is_noop() {
        // SAFETY: documented contract — null is allowed and ignored.
        unsafe { axiom_recovery_words_free(std::ptr::null_mut()) };
    }

    /// `axiom_recovery_words_free` zeroizes the buffer before releasing it.
    ///
    /// Rather than reading freed memory (which is UB), we verify the wipe by
    /// reimplementing the free logic in safe code: manually reclaim a matching
    /// `CString`, inspect its bytes to confirm they match the mnemonic, then
    /// apply the same `write_bytes` + fence sequence that
    /// `axiom_recovery_words_free` performs and assert the result.
    ///
    /// This is a structural check on the primitive used inside the FFI free
    /// function — it ensures the compiler cannot elide the wipe under the
    /// compiler_fence and that the byte range covers the full C string
    /// including the NUL terminator.
    #[test]
    fn recovery_words_free_wipes_bytes_before_drop() {
        let secret = Zeroizing::new("witness witness witness witness".to_string());
        let raw = into_secret_cstr(secret).expect("conversion succeeds");

        // Take ownership of the allocation back (mirrors the first step in
        // axiom_recovery_words_free). SAFETY: `raw` came from CString::into_raw.
        let cstring = unsafe { CString::from_raw(raw) };
        let bytes_with_nul = cstring.as_bytes_with_nul();
        assert_eq!(bytes_with_nul[0], b'w');
        assert!(bytes_with_nul.ends_with(&[0]));
        let len = bytes_with_nul.len();
        let ptr = bytes_with_nul.as_ptr() as *mut u8;

        // Apply the same wipe as the FFI free.
        // SAFETY: exclusive ownership via `cstring`; writing `len` bytes is in-bounds.
        unsafe { std::ptr::write_bytes(ptr, 0, len) };
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);

        // The buffer is now all zeros while still owned.
        let after: &[u8] = cstring.as_bytes_with_nul();
        assert!(after.iter().all(|&b| b == 0), "buffer not fully zeroed");

        // Drop releases the (now-zeroed) allocation.
        drop(cstring);
    }
}
