//! Vault operations for FFI
//!
//! Wraps core vault operations into FFI-safe async functions.

use std::ffi::CString;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

use axiomvault_common::{VaultId, VaultPath};
use axiomvault_crypto::KdfParams;
use axiomvault_vault::{
    check_migration_needed, check_vault_health, check_vault_structure, MigrationRegistry,
    MigrationStatus, VaultConfig, VaultManager as CoreVaultManager, VaultOperations, VaultVersion,
};

use crate::error::{FFIError, FFIResult};
use crate::types::{FFIVaultHandle, FFIVaultInfo, VaultHandleData};

/// FFI-safe vault manager
pub struct VaultManager {
    core: CoreVaultManager,
}

impl VaultManager {
    /// Create a new vault manager
    pub fn new() -> Self {
        Self {
            core: CoreVaultManager::new(),
        }
    }

    /// Get the core manager
    pub fn core(&self) -> &CoreVaultManager {
        &self.core
    }
}

impl Default for VaultManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a new vault at the specified path.
pub async fn create_vault(path: &str, password: &str) -> FFIResult<FFIVaultHandle> {
    let path_obj = Path::new(path);

    // Ensure the path is absolute
    let abs_path = if path_obj.is_absolute() {
        path_obj.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| FFIError::IOError(e.to_string()))?
            .join(path_obj)
    };

    let abs_path_str = abs_path.to_string_lossy().to_string();

    // Create vault ID from path name
    let vault_name = abs_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("vault");

    let vault_id = VaultId::new(vault_name).map_err(|e| FFIError::VaultError(e.to_string()))?;

    // Create provider config for local storage
    let provider_config = serde_json::json!({
        "root": abs_path_str
    });

    let manager = CoreVaultManager::new();

    let creation = manager
        .create_vault(
            vault_id,
            password.as_bytes(),
            "local",
            provider_config,
            KdfParams::moderate(),
        )
        .await
        .map_err(|e| FFIError::VaultError(e.to_string()))?;

    // Note: recovery_words from creation are available but not returned
    // through this simple FFI. Use axiom_vault_create_with_recovery instead.

    Ok(VaultHandleData {
        session: Arc::new(RwLock::new(creation.session)),
        path: abs_path_str,
        recovery_words: Some(creation.recovery_words),
    })
}

/// Open an existing vault at the specified path.
pub async fn open_vault(path: &str, password: &str) -> FFIResult<FFIVaultHandle> {
    let path_obj = Path::new(path);

    // Ensure the path is absolute
    let abs_path = if path_obj.is_absolute() {
        path_obj.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| FFIError::IOError(e.to_string()))?
            .join(path_obj)
    };

    let abs_path_str = abs_path.to_string_lossy().to_string();

    // Create provider config for local storage
    let provider_config = serde_json::json!({
        "root": abs_path_str
    });

    let manager = CoreVaultManager::new();

    let session = manager
        .open_vault("local", provider_config, password.as_bytes())
        .await
        .map_err(|e| FFIError::VaultError(e.to_string()))?;

    Ok(VaultHandleData {
        session: Arc::new(RwLock::new(session)),
        path: abs_path_str,
        recovery_words: None,
    })
}

/// Get information about an open vault.
pub fn get_vault_info(handle: &FFIVaultHandle) -> FFIResult<FFIVaultInfo> {
    // Block on getting session info
    let runtime =
        crate::runtime::get_runtime().map_err(|e| FFIError::RuntimeError(e.to_string()))?;

    runtime.block_on(async {
        let session = handle.session.read().await;

        // Build both CStrings before calling .into_raw() to avoid leaking
        // the first one if the second construction fails.
        let vault_id_cstr = CString::new(session.vault_id().as_str())
            .map_err(|_| FFIError::StringConversionError)?;
        let root_path_cstr =
            CString::new(handle.path.clone()).map_err(|_| FFIError::StringConversionError)?;

        let vault_id_str = vault_id_cstr.into_raw();
        let root_path_str = root_path_cstr.into_raw();

        // Count files in tree
        let tree = session.tree().read().await;
        let file_count = tree.count_files() as i32;
        let total_size = tree.total_size() as i64;

        Ok(FFIVaultInfo {
            vault_id: vault_id_str as *const _,
            root_path: root_path_str as *const _,
            file_count,
            total_size,
            version: 1,
        })
    })
}

/// List vault contents at the specified path.
pub async fn list_vault(handle: &FFIVaultHandle, path: &str) -> FFIResult<String> {
    let session = handle.session.read().await;
    let ops = VaultOperations::new(&session).map_err(|e| FFIError::VaultError(e.to_string()))?;

    let vault_path = VaultPath::parse(path).map_err(|e| FFIError::VaultError(e.to_string()))?;

    let entries = ops
        .list_directory(&vault_path)
        .await
        .map_err(|e| FFIError::VaultError(e.to_string()))?;

    // Convert to JSON
    let json_entries: Vec<serde_json::Value> = entries
        .into_iter()
        .map(|(name, is_dir, size)| {
            serde_json::json!({
                "name": name,
                "is_directory": is_dir,
                "size": size
            })
        })
        .collect();

    serde_json::to_string(&json_entries).map_err(|e| FFIError::VaultError(e.to_string()))
}

/// Add a file to the vault.
pub async fn add_file(
    handle: &FFIVaultHandle,
    local_path: &str,
    vault_path: &str,
) -> FFIResult<()> {
    // Read the local file
    let content = tokio::fs::read(local_path)
        .await
        .map_err(|e| FFIError::IOError(e.to_string()))?;

    let session = handle.session.read().await;
    let ops = VaultOperations::new(&session).map_err(|e| FFIError::VaultError(e.to_string()))?;

    let vpath = VaultPath::parse(vault_path).map_err(|e| FFIError::VaultError(e.to_string()))?;

    ops.create_file(&vpath, &content)
        .await
        .map_err(|e| FFIError::VaultError(e.to_string()))
}

/// Extract a file from the vault.
pub async fn extract_file(
    handle: &FFIVaultHandle,
    vault_path: &str,
    local_path: &str,
) -> FFIResult<()> {
    let session = handle.session.read().await;
    let ops = VaultOperations::new(&session).map_err(|e| FFIError::VaultError(e.to_string()))?;

    let vpath = VaultPath::parse(vault_path).map_err(|e| FFIError::VaultError(e.to_string()))?;

    let content = ops
        .read_file(&vpath)
        .await
        .map_err(|e| FFIError::VaultError(e.to_string()))?;

    // Write to local file
    tokio::fs::write(local_path, content)
        .await
        .map_err(|e| FFIError::IOError(e.to_string()))
}

/// Create a directory in the vault.
pub async fn create_directory(handle: &FFIVaultHandle, vault_path: &str) -> FFIResult<()> {
    let session = handle.session.read().await;
    let ops = VaultOperations::new(&session).map_err(|e| FFIError::VaultError(e.to_string()))?;

    let vpath = VaultPath::parse(vault_path).map_err(|e| FFIError::VaultError(e.to_string()))?;

    ops.create_directory(&vpath)
        .await
        .map_err(|e| FFIError::VaultError(e.to_string()))
}

/// Remove a file or directory from the vault.
pub async fn remove_entry(handle: &FFIVaultHandle, vault_path: &str) -> FFIResult<()> {
    let session = handle.session.read().await;
    let ops = VaultOperations::new(&session).map_err(|e| FFIError::VaultError(e.to_string()))?;

    let vpath = VaultPath::parse(vault_path).map_err(|e| FFIError::VaultError(e.to_string()))?;

    // Check if it's a file or directory
    let (_, is_dir, _) = ops
        .metadata(&vpath)
        .await
        .map_err(|e| FFIError::VaultError(e.to_string()))?;

    if is_dir {
        ops.delete_directory(&vpath)
            .await
            .map_err(|e| FFIError::VaultError(e.to_string()))
    } else {
        ops.delete_file(&vpath)
            .await
            .map_err(|e| FFIError::VaultError(e.to_string()))
    }
}

/// Check migration status for a vault at the given path.
///
/// Returns:
/// - 0: up to date
/// - 1: needs migration
/// - -1 (via error): incompatible or error
pub fn check_migration(path: &str) -> FFIResult<i32> {
    let vault_path = std::path::Path::new(path);
    let config_path = vault_path.join("vault.config");

    let config_bytes = std::fs::read(&config_path).map_err(|e| FFIError::IOError(e.to_string()))?;
    let config =
        VaultConfig::from_bytes(&config_bytes).map_err(|e| FFIError::VaultError(e.to_string()))?;

    match check_migration_needed(&config) {
        MigrationStatus::UpToDate => Ok(0),
        MigrationStatus::NeedsMigration { .. } => Ok(1),
        MigrationStatus::Incompatible { version } => Err(FFIError::VaultError(format!(
            "Incompatible vault version: {}",
            version
        ))),
    }
}

/// Run migrations on a vault at the given path.
pub fn run_migration(path: &str, _password: &str) -> FFIResult<()> {
    let vault_path = std::path::Path::new(path);
    let config_path = vault_path.join("vault.config");

    let config_bytes = std::fs::read(&config_path).map_err(|e| FFIError::IOError(e.to_string()))?;
    let mut config =
        VaultConfig::from_bytes(&config_bytes).map_err(|e| FFIError::VaultError(e.to_string()))?;

    let registry = MigrationRegistry::default();
    let target = VaultVersion::CURRENT;

    registry
        .migrate(vault_path, &mut config, &target)
        .map_err(|e| FFIError::VaultError(e.to_string()))
}

/// Change the vault password.
pub async fn change_password(
    handle: &FFIVaultHandle,
    old_password: &str,
    new_password: &str,
) -> FFIResult<()> {
    let mut session = handle.session.write().await;

    session
        .change_password(old_password.as_bytes(), new_password.as_bytes())
        .map_err(|e| FFIError::VaultError(e.to_string()))?;

    // Save the updated config
    let manager = CoreVaultManager::new();
    manager
        .save_config(&session)
        .await
        .map_err(|e| FFIError::VaultError(e.to_string()))
}

/// Show recovery key for an open vault.
pub async fn show_recovery_key(handle: &FFIVaultHandle) -> FFIResult<String> {
    let session = handle.session.read().await;

    let master_key = session
        .master_key()
        .map_err(|e| FFIError::VaultError(e.to_string()))?;

    let recovery_key = session
        .config()
        .decrypt_recovery_key(master_key)
        .map_err(|e| FFIError::VaultError(e.to_string()))?;

    recovery_key
        .to_mnemonic()
        .map_err(|e| FFIError::VaultError(e.to_string()))
}

/// Reset vault password using recovery key words.
pub async fn reset_password(
    path: &str,
    recovery_words: &str,
    new_password: &str,
) -> FFIResult<FFIVaultHandle> {
    let path_obj = Path::new(path);

    let abs_path = if path_obj.is_absolute() {
        path_obj.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| FFIError::IOError(e.to_string()))?
            .join(path_obj)
    };

    let abs_path_str = abs_path.to_string_lossy().to_string();

    let provider_config = serde_json::json!({
        "root": abs_path_str
    });

    let manager = CoreVaultManager::new();

    let session = manager
        .recover_vault(
            "local",
            provider_config,
            recovery_words,
            new_password.as_bytes(),
        )
        .await
        .map_err(|e| FFIError::VaultError(e.to_string()))?;

    Ok(VaultHandleData {
        session: Arc::new(RwLock::new(session)),
        path: abs_path_str,
        recovery_words: None,
    })
}

/// Run a health check on a vault. Returns JSON report.
///
/// If `password` is `None`, runs a shallow structure-only check.
/// If `password` is `Some`, runs a full integrity check.
pub async fn health_check(path: &str, password: Option<&str>) -> FFIResult<String> {
    let path_obj = Path::new(path);

    let abs_path = if path_obj.is_absolute() {
        path_obj.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| FFIError::IOError(e.to_string()))?
            .join(path_obj)
    };

    let abs_path_str = abs_path.to_string_lossy().to_string();

    let provider_config = serde_json::json!({
        "root": abs_path_str
    });

    let manager = CoreVaultManager::new();
    let provider = manager
        .registry()
        .resolve("local", provider_config.clone())
        .map_err(|e| FFIError::VaultError(e.to_string()))?;

    match password {
        None => {
            let report = check_vault_structure(provider.as_ref(), &abs_path_str)
                .await
                .map_err(|e| FFIError::VaultError(e.to_string()))?;
            Ok(report.to_json())
        }
        Some(pw) => {
            let session = manager
                .open_vault("local", provider_config, pw.as_bytes())
                .await
                .map_err(|e| FFIError::VaultError(e.to_string()))?;

            let master_key = session
                .master_key()
                .map_err(|e| FFIError::VaultError(e.to_string()))?;

            let report = check_vault_health(
                provider.as_ref(),
                session.config(),
                master_key,
                &abs_path_str,
            )
            .await
            .map_err(|e| FFIError::VaultError(e.to_string()))?;
            Ok(report.to_json())
        }
    }
}
