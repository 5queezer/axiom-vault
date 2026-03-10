//! Tauri command handlers for vault operations.
//!
//! These are thin wrappers that delegate to the shared `AppService` facade.
//! Business logic lives in `axiomvault-app`, not here.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::State;
use tracing::info;

use axiomvault_app::{
    CreateVaultParams, DirectoryEntryDto, LocalIndex, OpenVaultParams, VaultInfoDto,
};

use crate::state::AppState;

/// Vault information for the frontend (extends core DTO with mount info).
#[derive(Debug, Clone, Serialize)]
pub struct VaultInfo {
    pub id: String,
    pub provider_type: String,
    pub is_mounted: bool,
    pub mount_point: Option<String>,
}

impl VaultInfo {
    fn from_dto(dto: &VaultInfoDto, mount_point: Option<String>) -> Self {
        Self {
            id: dto.id.clone(),
            provider_type: dto.provider_type.clone(),
            is_mounted: mount_point.is_some(),
            mount_point,
        }
    }
}

/// Result of vault creation, including recovery words.
#[derive(Debug, Clone, Serialize)]
pub struct VaultCreationResult {
    pub vault: VaultInfo,
    pub recovery_words: String,
}

/// File entry for the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size: Option<u64>,
}

impl From<DirectoryEntryDto> for FileEntry {
    fn from(dto: DirectoryEntryDto) -> Self {
        Self {
            name: dto.name,
            path: dto.path,
            is_directory: dto.is_directory,
            size: dto.size,
        }
    }
}

/// Create a new vault.
#[tauri::command]
pub async fn create_vault(
    state: State<'_, Arc<AppState>>,
    id: String,
    password: String,
    provider_type: String,
) -> Result<VaultCreationResult, String> {
    info!("Creating vault: {}", id);

    let storage_root = state.data_dir.join(&id).join("storage");
    let provider_config = serde_json::json!({
        "root": storage_root.to_string_lossy()
    });

    let result = state
        .service
        .create_vault(CreateVaultParams {
            vault_id: id.clone(),
            password,
            provider_type,
            provider_config,
        })
        .await
        .map_err(|e| e.to_string())?;

    // Attach local index for metadata caching.
    let index_path = state.data_dir.join(format!("{}.db", id));
    let index = LocalIndex::open(&index_path).map_err(|e| e.to_string())?;
    state
        .service
        .set_local_index(index)
        .await
        .map_err(|e| e.to_string())?;

    info!("Vault created successfully");
    Ok(VaultCreationResult {
        vault: VaultInfo::from_dto(&result.info, None),
        recovery_words: result.recovery_words,
    })
}

/// Unlock an existing vault.
#[tauri::command]
pub async fn unlock_vault(
    state: State<'_, Arc<AppState>>,
    id: String,
    password: String,
) -> Result<VaultInfo, String> {
    info!("Unlocking vault: {}", id);

    let storage_root = state.data_dir.join(&id).join("storage");
    let provider_config = serde_json::json!({
        "root": storage_root.to_string_lossy()
    });

    let info = state
        .service
        .open_vault(OpenVaultParams {
            password,
            provider_type: "local".to_string(),
            provider_config,
        })
        .await
        .map_err(|e| e.to_string())?;

    // Attach local index for metadata caching.
    let index_path = state.data_dir.join(format!("{}.db", id));
    let index = LocalIndex::open(&index_path).map_err(|e| e.to_string())?;
    state
        .service
        .set_local_index(index)
        .await
        .map_err(|e| e.to_string())?;

    Ok(VaultInfo::from_dto(&info, None))
}

/// Lock a vault.
#[tauri::command]
pub async fn lock_vault(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    info!("Locking vault: {}", id);

    {
        let mut mounts = state.mounts.write().await;
        mounts.remove(&id);
    }

    state.service.lock_vault().await.map_err(|e| e.to_string())
}

/// Mount a vault as FUSE filesystem.
#[tauri::command]
pub async fn mount_vault(
    state: State<'_, Arc<AppState>>,
    id: String,
    mount_point: String,
) -> Result<VaultInfo, String> {
    info!("Mounting vault");

    let mount_path = PathBuf::from(&mount_point);
    if !mount_path.exists() {
        std::fs::create_dir_all(&mount_path).map_err(|e| e.to_string())?;
    }

    let session = state
        .service
        .vault_session()
        .await
        .map_err(|e| e.to_string())?;

    let runtime_handle = tokio::runtime::Handle::current();
    let mount_handle = axiomvault_fuse::mount::mount(
        session,
        &mount_path,
        axiomvault_fuse::MountOptions::default(),
        runtime_handle,
    )
    .map_err(|e| e.to_string())?;

    {
        let mut mounts = state.mounts.write().await;
        mounts.insert(id.clone(), crate::state::MountState { mount_handle });
    }

    let vault_info = state
        .service
        .vault_info()
        .await
        .map_err(|e| e.to_string())?;

    info!("Vault mounted successfully");
    Ok(VaultInfo::from_dto(&vault_info, Some(mount_point)))
}

/// Unmount a vault.
#[tauri::command]
pub async fn unmount_vault(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    info!("Unmounting vault: {}", id);

    let mut mounts = state.mounts.write().await;
    if mounts.remove(&id).is_some() {
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
    #[allow(unused_variables)] id: String,
    path: String,
) -> Result<Vec<FileEntry>, String> {
    let entries = state
        .service
        .list_directory(&path)
        .await
        .map_err(|e| e.to_string())?;

    Ok(entries.into_iter().map(FileEntry::from).collect())
}

/// Create a new file.
#[tauri::command]
pub async fn create_file(
    state: State<'_, Arc<AppState>>,
    #[allow(unused_variables)] vault_id: String,
    path: String,
    content: Vec<u8>,
) -> Result<(), String> {
    info!("Creating file");
    state
        .service
        .create_file(&path, &content)
        .await
        .map_err(|e| e.to_string())
}

/// Read a file's content.
#[tauri::command]
pub async fn read_file(
    state: State<'_, Arc<AppState>>,
    #[allow(unused_variables)] vault_id: String,
    path: String,
) -> Result<Vec<u8>, String> {
    state
        .service
        .read_file(&path)
        .await
        .map_err(|e| e.to_string())
}

/// Update a file's content.
#[tauri::command]
pub async fn update_file(
    state: State<'_, Arc<AppState>>,
    #[allow(unused_variables)] vault_id: String,
    path: String,
    content: Vec<u8>,
) -> Result<(), String> {
    info!("Updating file");
    state
        .service
        .update_file(&path, &content)
        .await
        .map_err(|e| e.to_string())
}

/// Delete a file.
#[tauri::command]
pub async fn delete_file(
    state: State<'_, Arc<AppState>>,
    #[allow(unused_variables)] vault_id: String,
    path: String,
) -> Result<(), String> {
    info!("Deleting file");
    state
        .service
        .delete_file(&path)
        .await
        .map_err(|e| e.to_string())
}

/// Create a directory.
#[tauri::command]
pub async fn create_directory(
    state: State<'_, Arc<AppState>>,
    #[allow(unused_variables)] vault_id: String,
    path: String,
) -> Result<(), String> {
    info!("Creating directory");
    state
        .service
        .create_directory(&path)
        .await
        .map_err(|e| e.to_string())
}

/// Delete an empty directory.
#[tauri::command]
pub async fn delete_directory(
    state: State<'_, Arc<AppState>>,
    #[allow(unused_variables)] vault_id: String,
    path: String,
) -> Result<(), String> {
    info!("Deleting directory");
    state
        .service
        .delete_directory(&path)
        .await
        .map_err(|e| e.to_string())
}

/// Get FUSE availability information.
#[tauri::command]
pub fn get_fuse_info() -> String {
    axiomvault_fuse::mount::fuse_info()
}

/// List all open vaults.
#[tauri::command]
pub async fn list_vaults(state: State<'_, Arc<AppState>>) -> Result<Vec<VaultInfo>, String> {
    match state.service.vault_info().await {
        Ok(info) => {
            let mounts = state.mounts.read().await;
            let mount_point = mounts
                .get(&info.id)
                .map(|m| m.mount_handle.mount_point().to_string_lossy().to_string());
            Ok(vec![VaultInfo::from_dto(&info, mount_point)])
        }
        Err(_) => Ok(vec![]),
    }
}
