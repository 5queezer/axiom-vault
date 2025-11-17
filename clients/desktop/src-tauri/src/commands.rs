//! Tauri command handlers for vault operations.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;
use tracing::{error, info};

use axiomvault_common::{VaultId, VaultPath};
use axiomvault_crypto::KdfParams;
use axiomvault_fuse::MountOptions;
use axiomvault_fuse::mount::mount as mount_vault_fuse;
use axiomvault_storage::{MemoryProvider, StorageProvider};
use axiomvault_vault::{VaultConfig, VaultOperations, VaultSession};

use crate::local_index::{IndexEntry, LocalIndex};
use crate::state::{AppState, OpenVault};

/// Vault information for the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct VaultInfo {
    pub id: String,
    pub provider_type: String,
    pub is_mounted: bool,
    pub mount_point: Option<String>,
}

/// File entry for the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size: Option<u64>,
}

/// Error response.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorResponse {
    pub message: String,
}

impl From<axiomvault_common::Error> for ErrorResponse {
    fn from(err: axiomvault_common::Error) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}

/// Helper to create provider, session, and register vault in state.
async fn setup_vault_session(
    state: &Arc<AppState>,
    id: &str,
    config: VaultConfig,
    password: &str,
    init_index_metadata: bool,
) -> Result<VaultInfo, String> {
    let provider_type = config.provider_type.clone();

    // For now, use memory provider for testing
    let provider = Arc::new(MemoryProvider::new());
    provider
        .create_dir(&VaultPath::parse("/d").unwrap())
        .await
        .map_err(|e| e.to_string())?;

    let session =
        VaultSession::unlock(config, password.as_bytes(), provider).map_err(|e| e.to_string())?;

    // Open local index
    let index_path = state.data_dir.join(format!("{}.db", id));
    let index = LocalIndex::open(&index_path).map_err(|e| e.to_string())?;

    if init_index_metadata {
        index
            .set_metadata("vault_id", id)
            .map_err(|e| e.to_string())?;
    }

    let open_vault = OpenVault {
        session: Arc::new(session),
        index: Arc::new(index),
        config_path: state.data_dir.join(format!("{}.json", id)),
        mount_handle: None,
    };

    let vault_info = VaultInfo {
        id: id.to_string(),
        provider_type,
        is_mounted: false,
        mount_point: None,
    };

    {
        let mut vaults = state.vaults.write().await;
        vaults.insert(id.to_string(), open_vault);
    }

    Ok(vault_info)
}

/// Create a new vault.
#[tauri::command]
pub async fn create_vault(
    state: State<'_, Arc<AppState>>,
    id: String,
    password: String,
    provider_type: String,
) -> Result<VaultInfo, String> {
    info!("Creating vault: {}", id);

    let vault_id = VaultId::new(&id).map_err(|e| e.to_string())?;
    let kdf_params = KdfParams::moderate();

    let config = VaultConfig::new(
        vault_id,
        password.as_bytes(),
        &provider_type,
        serde_json::Value::Null,
        kdf_params,
    )
    .map_err(|e| e.to_string())?;

    let vault_info = setup_vault_session(&state, &id, config, &password, true).await?;

    info!("Vault created successfully");
    Ok(vault_info)
}

/// Unlock an existing vault.
#[tauri::command]
pub async fn unlock_vault(
    state: State<'_, Arc<AppState>>,
    id: String,
    password: String,
) -> Result<VaultInfo, String> {
    info!("Unlocking vault: {}", id);

    // Load config from disk
    let config_path = state.data_dir.join(format!("{}.json", id));
    if !config_path.exists() {
        return Err("Vault not found".to_string());
    }

    let config_data = std::fs::read_to_string(&config_path).map_err(|e| e.to_string())?;
    let config: VaultConfig = serde_json::from_str(&config_data).map_err(|e| e.to_string())?;

    let vault_info = setup_vault_session(&state, &id, config, &password, false).await?;

    info!("Vault unlocked successfully");
    Ok(vault_info)
}

/// Lock a vault.
#[tauri::command]
pub async fn lock_vault(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    info!("Locking vault: {}", id);

    let mut vaults = state.vaults.write().await;
    if vaults.remove(&id).is_some() {
        info!("Vault locked successfully");
        Ok(())
    } else {
        Err("Vault not open".to_string())
    }
}

/// Mount a vault as FUSE filesystem.
#[tauri::command]
pub async fn mount_vault(
    state: State<'_, Arc<AppState>>,
    id: String,
    mount_point: String,
) -> Result<VaultInfo, String> {
    info!("Mounting vault {} at {}", id, mount_point);

    let mount_path = PathBuf::from(&mount_point);
    if !mount_path.exists() {
        std::fs::create_dir_all(&mount_path).map_err(|e| e.to_string())?;
    }

    let mut vaults = state.vaults.write().await;
    let vault = vaults.get_mut(&id).ok_or("Vault not open")?;

    let runtime_handle = tokio::runtime::Handle::current();
    let mount_handle = mount_vault_fuse(
        vault.session.clone(),
        &mount_path,
        MountOptions::default(),
        runtime_handle,
    )
    .map_err(|e| e.to_string())?;

    vault.mount_handle = Some(mount_handle);

    let vault_info = VaultInfo {
        id,
        provider_type: vault.session.config().provider_type.clone(),
        is_mounted: true,
        mount_point: Some(mount_point),
    };

    info!("Vault mounted successfully");
    Ok(vault_info)
}

/// Unmount a vault.
#[tauri::command]
pub async fn unmount_vault(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    info!("Unmounting vault: {}", id);

    let mut vaults = state.vaults.write().await;
    let vault = vaults.get_mut(&id).ok_or("Vault not open")?;

    if let Some(handle) = vault.mount_handle.take() {
        drop(handle);
        info!("Vault unmounted successfully");
        Ok(())
    } else {
        Err("Vault not mounted".to_string())
    }
}

/// List files in a vault directory.
#[tauri::command]
pub async fn list_files(
    state: State<'_, Arc<AppState>>,
    id: String,
    path: String,
) -> Result<Vec<FileEntry>, String> {
    let vaults = state.vaults.read().await;
    let vault = vaults.get(&id).ok_or("Vault not open")?;

    let ops = VaultOperations::new(&vault.session).map_err(|e| e.to_string())?;
    let vault_path = VaultPath::parse(&path).map_err(|e| e.to_string())?;

    let entries = ops
        .list_directory(&vault_path)
        .await
        .map_err(|e| e.to_string())?;

    let file_entries: Vec<FileEntry> = entries
        .into_iter()
        .map(|(name, is_dir, size)| {
            let full_path = if path == "/" {
                format!("/{}", name)
            } else {
                format!("{}/{}", path, name)
            };
            FileEntry {
                name,
                path: full_path,
                is_directory: is_dir,
                size,
            }
        })
        .collect();

    Ok(file_entries)
}

/// Create a new file.
#[tauri::command]
pub async fn create_file(
    state: State<'_, Arc<AppState>>,
    vault_id: String,
    path: String,
    content: Vec<u8>,
) -> Result<(), String> {
    info!("Creating file: {}", path);

    let vaults = state.vaults.read().await;
    let vault = vaults.get(&vault_id).ok_or("Vault not open")?;

    let ops = VaultOperations::new(&vault.session).map_err(|e| e.to_string())?;
    let vault_path = VaultPath::parse(&path).map_err(|e| e.to_string())?;

    ops.create_file(&vault_path, &content)
        .await
        .map_err(|e| e.to_string())?;

    // Update local index
    let entry = IndexEntry {
        path: path.clone(),
        encrypted_name: String::new(), // Will be updated from tree
        is_directory: false,
        size: Some(content.len() as u64),
        modified_at: chrono::Utc::now().timestamp(),
        etag: None,
    };
    vault.index.upsert_entry(&entry).map_err(|e| e.to_string())?;

    info!("File created successfully");
    Ok(())
}

/// Read a file's content.
#[tauri::command]
pub async fn read_file(
    state: State<'_, Arc<AppState>>,
    vault_id: String,
    path: String,
) -> Result<Vec<u8>, String> {
    let vaults = state.vaults.read().await;
    let vault = vaults.get(&vault_id).ok_or("Vault not open")?;

    let ops = VaultOperations::new(&vault.session).map_err(|e| e.to_string())?;
    let vault_path = VaultPath::parse(&path).map_err(|e| e.to_string())?;

    ops.read_file(&vault_path)
        .await
        .map_err(|e| e.to_string())
}

/// Update a file's content.
#[tauri::command]
pub async fn update_file(
    state: State<'_, Arc<AppState>>,
    vault_id: String,
    path: String,
    content: Vec<u8>,
) -> Result<(), String> {
    info!("Updating file: {}", path);

    let vaults = state.vaults.read().await;
    let vault = vaults.get(&vault_id).ok_or("Vault not open")?;

    let ops = VaultOperations::new(&vault.session).map_err(|e| e.to_string())?;
    let vault_path = VaultPath::parse(&path).map_err(|e| e.to_string())?;

    ops.update_file(&vault_path, &content)
        .await
        .map_err(|e| e.to_string())?;

    info!("File updated successfully");
    Ok(())
}

/// Delete a file.
#[tauri::command]
pub async fn delete_file(
    state: State<'_, Arc<AppState>>,
    vault_id: String,
    path: String,
) -> Result<(), String> {
    info!("Deleting file: {}", path);

    let vaults = state.vaults.read().await;
    let vault = vaults.get(&vault_id).ok_or("Vault not open")?;

    let ops = VaultOperations::new(&vault.session).map_err(|e| e.to_string())?;
    let vault_path = VaultPath::parse(&path).map_err(|e| e.to_string())?;

    ops.delete_file(&vault_path)
        .await
        .map_err(|e| e.to_string())?;

    vault.index.delete_entry(&path).map_err(|e| e.to_string())?;

    info!("File deleted successfully");
    Ok(())
}

/// Create a directory.
#[tauri::command]
pub async fn create_directory(
    state: State<'_, Arc<AppState>>,
    vault_id: String,
    path: String,
) -> Result<(), String> {
    info!("Creating directory: {}", path);

    let vaults = state.vaults.read().await;
    let vault = vaults.get(&vault_id).ok_or("Vault not open")?;

    let ops = VaultOperations::new(&vault.session).map_err(|e| e.to_string())?;
    let vault_path = VaultPath::parse(&path).map_err(|e| e.to_string())?;

    ops.create_directory(&vault_path)
        .await
        .map_err(|e| e.to_string())?;

    let entry = IndexEntry {
        path: path.clone(),
        encrypted_name: String::new(),
        is_directory: true,
        size: None,
        modified_at: chrono::Utc::now().timestamp(),
        etag: None,
    };
    vault.index.upsert_entry(&entry).map_err(|e| e.to_string())?;

    info!("Directory created successfully");
    Ok(())
}

/// Delete an empty directory.
#[tauri::command]
pub async fn delete_directory(
    state: State<'_, Arc<AppState>>,
    vault_id: String,
    path: String,
) -> Result<(), String> {
    info!("Deleting directory: {}", path);

    let vaults = state.vaults.read().await;
    let vault = vaults.get(&vault_id).ok_or("Vault not open")?;

    let ops = VaultOperations::new(&vault.session).map_err(|e| e.to_string())?;
    let vault_path = VaultPath::parse(&path).map_err(|e| e.to_string())?;

    ops.delete_directory(&vault_path)
        .await
        .map_err(|e| e.to_string())?;

    vault.index.delete_entry(&path).map_err(|e| e.to_string())?;

    info!("Directory deleted successfully");
    Ok(())
}

/// Get FUSE availability information.
#[tauri::command]
pub fn get_fuse_info() -> String {
    axiomvault_fuse::mount::fuse_info()
}

/// List all open vaults.
#[tauri::command]
pub async fn list_vaults(state: State<'_, Arc<AppState>>) -> Result<Vec<VaultInfo>, String> {
    let vaults = state.vaults.read().await;

    let vault_list: Vec<VaultInfo> = vaults
        .iter()
        .map(|(id, vault)| VaultInfo {
            id: id.clone(),
            provider_type: vault.session.config().provider_type.clone(),
            is_mounted: vault.mount_handle.is_some(),
            mount_point: vault
                .mount_handle
                .as_ref()
                .map(|h| h.mount_point().to_string_lossy().to_string()),
        })
        .collect();

    Ok(vault_list)
}
