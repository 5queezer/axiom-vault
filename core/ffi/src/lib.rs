//! FFI bindings for AxiomVault core
//!
//! This module provides C-ABI compatible functions for mobile platforms (iOS/Android).
//! All functions are designed to be safe to call from foreign code.

#![allow(clippy::missing_safety_doc)]

pub mod error;
pub mod runtime;
pub mod types;
pub mod vault_ops;

use std::ffi::{c_char, c_int, CStr, CString};
use std::ptr;

use crate::error::FFIError;
use crate::runtime::get_runtime;
use crate::types::{FFIVaultHandle, FFIVaultInfo};

/// Initialize the FFI layer. Must be called before any other FFI functions.
///
/// # Safety
/// This function is safe to call from foreign code.
#[no_mangle]
pub extern "C" fn axiom_init() -> c_int {
    // Initialize tracing
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();

    // Initialize runtime
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
    if path.is_null() || password.is_null() {
        error::set_last_error(FFIError::NullPointer("path or password is null".into()));
        return ptr::null_mut();
    }

    let path_str = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => {
            error::set_last_error(FFIError::InvalidUtf8("path".into()));
            return ptr::null_mut();
        }
    };

    let password_str = match CStr::from_ptr(password).to_str() {
        Ok(s) => s,
        Err(_) => {
            error::set_last_error(FFIError::InvalidUtf8("password".into()));
            return ptr::null_mut();
        }
    };

    let runtime = match get_runtime() {
        Ok(rt) => rt,
        Err(e) => {
            error::set_last_error(FFIError::RuntimeError(e.to_string()));
            return ptr::null_mut();
        }
    };

    match runtime.block_on(vault_ops::create_vault(path_str, password_str)) {
        Ok(handle) => Box::into_raw(Box::new(handle)),
        Err(e) => {
            error::set_last_error(e);
            ptr::null_mut()
        }
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
    if path.is_null() || password.is_null() {
        error::set_last_error(FFIError::NullPointer("path or password is null".into()));
        return ptr::null_mut();
    }

    let path_str = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => {
            error::set_last_error(FFIError::InvalidUtf8("path".into()));
            return ptr::null_mut();
        }
    };

    let password_str = match CStr::from_ptr(password).to_str() {
        Ok(s) => s,
        Err(_) => {
            error::set_last_error(FFIError::InvalidUtf8("password".into()));
            return ptr::null_mut();
        }
    };

    let runtime = match get_runtime() {
        Ok(rt) => rt,
        Err(e) => {
            error::set_last_error(FFIError::RuntimeError(e.to_string()));
            return ptr::null_mut();
        }
    };

    match runtime.block_on(vault_ops::open_vault(path_str, password_str)) {
        Ok(handle) => Box::into_raw(Box::new(handle)),
        Err(e) => {
            error::set_last_error(e);
            ptr::null_mut()
        }
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

    let _handle = Box::from_raw(handle);
    // Handle is dropped here, closing the vault
    0
}

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

    let handle = &*handle;
    match vault_ops::get_vault_info(handle) {
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
        // Free the internal CString pointers
        if !info.vault_id.is_null() {
            let _ = CString::from_raw(info.vault_id as *mut c_char);
        }
        if !info.root_path.is_null() {
            let _ = CString::from_raw(info.root_path as *mut c_char);
        }
    }
}

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
    if handle.is_null() || path.is_null() {
        error::set_last_error(FFIError::NullPointer("handle or path is null".into()));
        return ptr::null_mut();
    }

    let handle = &*handle;
    let path_str = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => {
            error::set_last_error(FFIError::InvalidUtf8("path".into()));
            return ptr::null_mut();
        }
    };

    let runtime = match get_runtime() {
        Ok(rt) => rt,
        Err(e) => {
            error::set_last_error(FFIError::RuntimeError(e.to_string()));
            return ptr::null_mut();
        }
    };

    match runtime.block_on(vault_ops::list_vault(handle, path_str)) {
        Ok(json) => match CString::new(json) {
            Ok(cstr) => cstr.into_raw(),
            Err(_) => {
                error::set_last_error(FFIError::StringConversionError);
                ptr::null_mut()
            }
        },
        Err(e) => {
            error::set_last_error(e);
            ptr::null_mut()
        }
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
    if handle.is_null() || local_path.is_null() || vault_path.is_null() {
        error::set_last_error(FFIError::NullPointer(
            "handle, local_path or vault_path is null".into(),
        ));
        return -1;
    }

    let handle = &*handle;
    let local_str = match CStr::from_ptr(local_path).to_str() {
        Ok(s) => s,
        Err(_) => {
            error::set_last_error(FFIError::InvalidUtf8("local_path".into()));
            return -1;
        }
    };

    let vault_str = match CStr::from_ptr(vault_path).to_str() {
        Ok(s) => s,
        Err(_) => {
            error::set_last_error(FFIError::InvalidUtf8("vault_path".into()));
            return -1;
        }
    };

    let runtime = match get_runtime() {
        Ok(rt) => rt,
        Err(e) => {
            error::set_last_error(FFIError::RuntimeError(e.to_string()));
            return -1;
        }
    };

    match runtime.block_on(vault_ops::add_file(handle, local_str, vault_str)) {
        Ok(_) => 0,
        Err(e) => {
            error::set_last_error(e);
            -1
        }
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
    if handle.is_null() || vault_path.is_null() || local_path.is_null() {
        error::set_last_error(FFIError::NullPointer(
            "handle, vault_path or local_path is null".into(),
        ));
        return -1;
    }

    let handle = &*handle;
    let vault_str = match CStr::from_ptr(vault_path).to_str() {
        Ok(s) => s,
        Err(_) => {
            error::set_last_error(FFIError::InvalidUtf8("vault_path".into()));
            return -1;
        }
    };

    let local_str = match CStr::from_ptr(local_path).to_str() {
        Ok(s) => s,
        Err(_) => {
            error::set_last_error(FFIError::InvalidUtf8("local_path".into()));
            return -1;
        }
    };

    let runtime = match get_runtime() {
        Ok(rt) => rt,
        Err(e) => {
            error::set_last_error(FFIError::RuntimeError(e.to_string()));
            return -1;
        }
    };

    match runtime.block_on(vault_ops::extract_file(handle, vault_str, local_str)) {
        Ok(_) => 0,
        Err(e) => {
            error::set_last_error(e);
            -1
        }
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
    if handle.is_null() || vault_path.is_null() {
        error::set_last_error(FFIError::NullPointer("handle or vault_path is null".into()));
        return -1;
    }

    let handle = &*handle;
    let vault_str = match CStr::from_ptr(vault_path).to_str() {
        Ok(s) => s,
        Err(_) => {
            error::set_last_error(FFIError::InvalidUtf8("vault_path".into()));
            return -1;
        }
    };

    let runtime = match get_runtime() {
        Ok(rt) => rt,
        Err(e) => {
            error::set_last_error(FFIError::RuntimeError(e.to_string()));
            return -1;
        }
    };

    match runtime.block_on(vault_ops::create_directory(handle, vault_str)) {
        Ok(_) => 0,
        Err(e) => {
            error::set_last_error(e);
            -1
        }
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
    if handle.is_null() || vault_path.is_null() {
        error::set_last_error(FFIError::NullPointer("handle or vault_path is null".into()));
        return -1;
    }

    let handle = &*handle;
    let vault_str = match CStr::from_ptr(vault_path).to_str() {
        Ok(s) => s,
        Err(_) => {
            error::set_last_error(FFIError::InvalidUtf8("vault_path".into()));
            return -1;
        }
    };

    let runtime = match get_runtime() {
        Ok(rt) => rt,
        Err(e) => {
            error::set_last_error(FFIError::RuntimeError(e.to_string()));
            return -1;
        }
    };

    match runtime.block_on(vault_ops::remove_entry(handle, vault_str)) {
        Ok(_) => 0,
        Err(e) => {
            error::set_last_error(e);
            -1
        }
    }
}

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
    if handle.is_null() || old_password.is_null() || new_password.is_null() {
        error::set_last_error(FFIError::NullPointer("handle or passwords are null".into()));
        return -1;
    }

    let handle = &*handle;
    let old_str = match CStr::from_ptr(old_password).to_str() {
        Ok(s) => s,
        Err(_) => {
            error::set_last_error(FFIError::InvalidUtf8("old_password".into()));
            return -1;
        }
    };

    let new_str = match CStr::from_ptr(new_password).to_str() {
        Ok(s) => s,
        Err(_) => {
            error::set_last_error(FFIError::InvalidUtf8("new_password".into()));
            return -1;
        }
    };

    let runtime = match get_runtime() {
        Ok(rt) => rt,
        Err(e) => {
            error::set_last_error(FFIError::RuntimeError(e.to_string()));
            return -1;
        }
    };

    match runtime.block_on(vault_ops::change_password(handle, old_str, new_str)) {
        Ok(_) => 0,
        Err(e) => {
            error::set_last_error(e);
            -1
        }
    }
}
