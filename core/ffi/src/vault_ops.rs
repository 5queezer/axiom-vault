//! Vault operations for FFI
//!
//! Delegates all operations to `AppService`, the shared application facade.

use std::ffi::CString;
use std::path::Path;

use axiomvault_app::{AppService, CreateVaultParams, OpenVaultParams, RecoverVaultParams};
use axiomvault_vault::{
    check_migration_needed, check_vault_health, check_vault_structure, MigrationRegistry,
    MigrationStatus, VaultConfig, VaultManager as CoreVaultManager, VaultVersion,
};

use crate::error::{FFIError, FFIResult};
use crate::types::{FFIVaultHandle, FFIVaultInfo};

/// Resolve an absolute path from a potentially relative one.
fn resolve_path(path: &str) -> FFIResult<String> {
    let path_obj = Path::new(path);
    let abs_path = if path_obj.is_absolute() {
        path_obj.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| FFIError::IOError(e.to_string()))?
            .join(path_obj)
    };
    Ok(abs_path.to_string_lossy().to_string())
}

/// Create a new vault at the specified path.
pub async fn create_vault(path: &str, password: &str) -> FFIResult<FFIVaultHandle> {
    let abs_path = resolve_path(path)?;

    // Derive vault ID from directory name.
    let vault_name = Path::new(&abs_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("vault");

    let provider_config = serde_json::json!({ "root": abs_path });

    let service = AppService::new();
    let result = service
        .create_vault(CreateVaultParams {
            vault_id: vault_name.to_string(),
            password: password.to_string(),
            provider_type: "local".to_string(),
            provider_config,
        })
        .await
        .map_err(FFIError::from)?;

    Ok(FFIVaultHandle {
        service,
        path: abs_path,
        recovery_words: std::sync::Mutex::new(Some(result.recovery_words)),
        event_task: std::sync::Mutex::new(None),
    })
}

/// Open an existing vault at the specified path.
pub async fn open_vault(path: &str, password: &str) -> FFIResult<FFIVaultHandle> {
    let abs_path = resolve_path(path)?;
    let provider_config = serde_json::json!({ "root": abs_path });

    let service = AppService::new();
    service
        .open_vault(OpenVaultParams {
            password: password.to_string(),
            provider_type: "local".to_string(),
            provider_config,
        })
        .await
        .map_err(FFIError::from)?;

    Ok(FFIVaultHandle {
        service,
        path: abs_path,
        recovery_words: std::sync::Mutex::new(None),
        event_task: std::sync::Mutex::new(None),
    })
}

/// Get information about an open vault.
pub fn get_vault_info(handle: &FFIVaultHandle) -> FFIResult<FFIVaultInfo> {
    let runtime =
        crate::runtime::get_runtime().map_err(|e| FFIError::RuntimeError(e.to_string()))?;

    runtime.block_on(async {
        let info = handle.service.vault_info().await.map_err(FFIError::from)?;

        let vault_id_cstr = CString::new(info.id).map_err(|_| FFIError::StringConversionError)?;
        let root_path_cstr =
            CString::new(handle.path.clone()).map_err(|_| FFIError::StringConversionError)?;

        Ok(FFIVaultInfo {
            vault_id: vault_id_cstr.into_raw() as *const _,
            root_path: root_path_cstr.into_raw() as *const _,
            file_count: 0, // Not cheaply available via AppService; list_directory if needed.
            total_size: 0,
            version: 1,
        })
    })
}

/// List vault contents at the specified path (returns JSON).
pub async fn list_vault(handle: &FFIVaultHandle, path: &str) -> FFIResult<String> {
    let entries = handle
        .service
        .list_directory(path)
        .await
        .map_err(FFIError::from)?;

    serde_json::to_string(&entries).map_err(|e| FFIError::VaultError(e.to_string()))
}

/// Add a file to the vault (import from local filesystem).
pub async fn add_file(
    handle: &FFIVaultHandle,
    local_path: &str,
    vault_path: &str,
) -> FFIResult<()> {
    handle
        .service
        .import_file(local_path, vault_path)
        .await
        .map_err(FFIError::from)
}

/// Extract a file from the vault (export to local filesystem).
pub async fn extract_file(
    handle: &FFIVaultHandle,
    vault_path: &str,
    local_path: &str,
) -> FFIResult<()> {
    handle
        .service
        .export_file(vault_path, local_path)
        .await
        .map_err(FFIError::from)
}

/// Create a directory in the vault.
pub async fn create_directory(handle: &FFIVaultHandle, vault_path: &str) -> FFIResult<()> {
    handle
        .service
        .create_directory(vault_path)
        .await
        .map_err(FFIError::from)
}

/// Remove a file or directory from the vault.
pub async fn remove_entry(handle: &FFIVaultHandle, vault_path: &str) -> FFIResult<()> {
    let meta = handle
        .service
        .metadata(vault_path)
        .await
        .map_err(FFIError::from)?;

    if meta.is_directory {
        handle
            .service
            .delete_directory(vault_path)
            .await
            .map_err(FFIError::from)
    } else {
        handle
            .service
            .delete_file(vault_path)
            .await
            .map_err(FFIError::from)
    }
}

/// Change the vault password.
pub async fn change_password(
    handle: &FFIVaultHandle,
    old_password: &str,
    new_password: &str,
) -> FFIResult<()> {
    handle
        .service
        .change_password(old_password, new_password)
        .await
        .map_err(FFIError::from)
}

/// Show recovery key for an open vault.
pub async fn show_recovery_key(handle: &FFIVaultHandle) -> FFIResult<String> {
    // Recovery key display requires direct session access (not in AppService).
    let session = handle
        .service
        .vault_session()
        .await
        .map_err(FFIError::from)?;

    let master_key = session
        .master_key()
        .map_err(|e| FFIError::VaultError(e.to_string()))?;

    let recovery_key = session
        .config()
        .decrypt_recovery_key(master_key)
        .map_err(|e| FFIError::VaultError(e.to_string()))?;

    recovery_key
        .to_mnemonic()
        .map(|z| String::from(&*z))
        .map_err(|e| FFIError::VaultError(e.to_string()))
}

/// Reset vault password using recovery key words.
pub async fn reset_password(
    path: &str,
    recovery_words: &str,
    new_password: &str,
) -> FFIResult<FFIVaultHandle> {
    let abs_path = resolve_path(path)?;
    let provider_config = serde_json::json!({ "root": abs_path });

    let service = AppService::new();
    service
        .recover_vault(RecoverVaultParams {
            recovery_words: recovery_words.to_string(),
            new_password: new_password.to_string(),
            provider_type: "local".to_string(),
            provider_config,
        })
        .await
        .map_err(FFIError::from)?;

    Ok(FFIVaultHandle {
        service,
        path: abs_path,
        recovery_words: std::sync::Mutex::new(None),
        event_task: std::sync::Mutex::new(None),
    })
}

/// Check migration status for a vault at the given path.
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

/// Run a health check on a vault. Returns JSON report.
pub async fn health_check(path: &str, password: Option<&str>) -> FFIResult<String> {
    let abs_path = resolve_path(path)?;
    let provider_config = serde_json::json!({ "root": abs_path });

    let manager = CoreVaultManager::new();
    let provider = manager
        .registry()
        .resolve("local", provider_config.clone())
        .map_err(|e| FFIError::VaultError(e.to_string()))?;

    match password {
        None => {
            let report = check_vault_structure(provider.as_ref(), &abs_path)
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

            let report =
                check_vault_health(provider.as_ref(), session.config(), master_key, &abs_path)
                    .await
                    .map_err(|e| FFIError::VaultError(e.to_string()))?;
            Ok(report.to_json())
        }
    }
}
