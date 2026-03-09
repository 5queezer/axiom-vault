//! Application facade — the single entry point for all vault operations.

use tokio::sync::RwLock;
use tracing::info;

use axiomvault_common::{VaultId, VaultPath};
use axiomvault_crypto::KdfParams;
use axiomvault_vault::{VaultManager, VaultOperations, VaultSession};

use crate::dto::*;
use crate::error::{AppError, AppResult};
use crate::events::{event_channel, AppEvent, EventReceiver, EventSender};

/// Application service wrapping all vault subsystems.
///
/// Thread-safe (`Send + Sync`) and designed to be shared via `Arc`.
/// All mutable state is behind interior locks.
pub struct AppService {
    manager: VaultManager,
    session: RwLock<Option<ActiveVault>>,
    event_tx: EventSender,
}

/// Internal state for an open vault.
struct ActiveVault {
    session: VaultSession,
    provider_type: String,
}

impl AppService {
    /// Create a new application service.
    pub fn new() -> Self {
        let (event_tx, _) = event_channel(64);
        Self {
            manager: VaultManager::new(),
            session: RwLock::new(None),
            event_tx,
        }
    }

    /// Subscribe to application events.
    pub fn subscribe(&self) -> EventReceiver {
        self.event_tx.subscribe()
    }

    /// Get a reference to the event sender (for bridging to FFI callbacks).
    pub fn event_sender(&self) -> &EventSender {
        &self.event_tx
    }

    fn emit(&self, event: AppEvent) {
        // Ignore send errors — no receivers is fine.
        let _ = self.event_tx.send(event);
    }

    // -- Vault lifecycle --

    /// Create a new vault.
    pub async fn create_vault(&self, params: CreateVaultParams) -> AppResult<VaultCreatedDto> {
        let vault_id =
            VaultId::new(&params.vault_id).map_err(|e| AppError::InvalidInput(e.to_string()))?;

        // Check if vault already exists.
        let exists = self
            .manager
            .vault_exists(&params.provider_type, params.provider_config.clone())
            .await
            .map_err(AppError::from)?;

        if exists {
            return Err(AppError::VaultAlreadyExists(params.vault_id));
        }

        let creation = self
            .manager
            .create_vault(
                vault_id,
                params.password.as_bytes(),
                &params.provider_type,
                params.provider_config,
                KdfParams::default(),
            )
            .await
            .map_err(AppError::from)?;

        let info = VaultInfoDto {
            id: creation.session.vault_id().to_string(),
            provider_type: params.provider_type.clone(),
            is_unlocked: true,
        };

        let dto = VaultCreatedDto {
            info: info.clone(),
            recovery_words: creation.recovery_words,
        };

        *self.session.write().await = Some(ActiveVault {
            session: creation.session,
            provider_type: params.provider_type,
        });

        self.emit(AppEvent::VaultCreated(info));

        info!(vault_id = %params.vault_id, "Vault created");
        Ok(dto)
    }

    /// Open an existing vault.
    pub async fn open_vault(&self, params: OpenVaultParams) -> AppResult<VaultInfoDto> {
        let session = self
            .manager
            .open_vault(
                &params.provider_type,
                params.provider_config,
                params.password.as_bytes(),
            )
            .await
            .map_err(AppError::from)?;

        let info = VaultInfoDto {
            id: session.vault_id().to_string(),
            provider_type: params.provider_type.clone(),
            is_unlocked: true,
        };

        *self.session.write().await = Some(ActiveVault {
            session,
            provider_type: params.provider_type,
        });

        self.emit(AppEvent::VaultOpened(info.clone()));

        info!(vault_id = %info.id, "Vault opened");
        Ok(info)
    }

    /// Recover a vault using recovery words.
    pub async fn recover_vault(&self, params: RecoverVaultParams) -> AppResult<VaultInfoDto> {
        let session = self
            .manager
            .recover_vault(
                &params.provider_type,
                params.provider_config,
                &params.recovery_words,
                params.new_password.as_bytes(),
            )
            .await
            .map_err(AppError::from)?;

        let info = VaultInfoDto {
            id: session.vault_id().to_string(),
            provider_type: params.provider_type.clone(),
            is_unlocked: true,
        };

        *self.session.write().await = Some(ActiveVault {
            session,
            provider_type: params.provider_type,
        });

        self.emit(AppEvent::VaultOpened(info.clone()));

        info!(vault_id = %info.id, "Vault recovered");
        Ok(info)
    }

    /// Lock the active vault (clears keys from memory).
    pub async fn lock_vault(&self) -> AppResult<()> {
        let mut guard = self.session.write().await;
        let active = guard.as_mut().ok_or(AppError::NoOpenVault)?;
        active.session.lock();
        drop(guard);

        self.emit(AppEvent::VaultLocked);
        info!("Vault locked");
        Ok(())
    }

    /// Close the active vault entirely.
    pub async fn close_vault(&self) -> AppResult<()> {
        let mut guard = self.session.write().await;
        if guard.is_none() {
            return Err(AppError::NoOpenVault);
        }
        *guard = None;
        drop(guard);

        self.emit(AppEvent::VaultClosed);
        info!("Vault closed");
        Ok(())
    }

    /// Change the vault password.
    pub async fn change_password(&self, old_password: &str, new_password: &str) -> AppResult<()> {
        let mut guard = self.session.write().await;
        let active = guard.as_mut().ok_or(AppError::NoOpenVault)?;

        active
            .session
            .change_password(old_password.as_bytes(), new_password.as_bytes())
            .map_err(AppError::from)?;

        // Persist the updated config.
        self.manager
            .save_config(&active.session)
            .await
            .map_err(AppError::from)?;

        self.emit(AppEvent::PasswordChanged);
        info!("Password changed");
        Ok(())
    }

    /// Check if a vault is currently open.
    pub async fn is_vault_open(&self) -> bool {
        self.session.read().await.is_some()
    }

    /// Get info about the current vault.
    pub async fn vault_info(&self) -> AppResult<VaultInfoDto> {
        let guard = self.session.read().await;
        let active = guard.as_ref().ok_or(AppError::NoOpenVault)?;
        Ok(VaultInfoDto {
            id: active.session.vault_id().to_string(),
            provider_type: active.provider_type.clone(),
            is_unlocked: active.session.is_active(),
        })
    }

    // -- File operations --

    /// Create a file in the vault.
    pub async fn create_file(&self, path: &str, content: &[u8]) -> AppResult<()> {
        let guard = self.session.read().await;
        let active = guard.as_ref().ok_or(AppError::NoOpenVault)?;
        let ops = VaultOperations::new(&active.session).map_err(AppError::from)?;
        let vault_path =
            VaultPath::parse(path).map_err(|e| AppError::InvalidInput(e.to_string()))?;

        ops.create_file(&vault_path, content)
            .await
            .map_err(AppError::from)?;

        drop(guard);
        self.emit(AppEvent::FileCreated {
            path: path.to_string(),
        });
        Ok(())
    }

    /// Read a file from the vault.
    pub async fn read_file(&self, path: &str) -> AppResult<Vec<u8>> {
        let guard = self.session.read().await;
        let active = guard.as_ref().ok_or(AppError::NoOpenVault)?;
        let ops = VaultOperations::new(&active.session).map_err(AppError::from)?;
        let vault_path =
            VaultPath::parse(path).map_err(|e| AppError::InvalidInput(e.to_string()))?;

        ops.read_file(&vault_path).await.map_err(AppError::from)
    }

    /// Update a file in the vault.
    pub async fn update_file(&self, path: &str, content: &[u8]) -> AppResult<()> {
        let guard = self.session.read().await;
        let active = guard.as_ref().ok_or(AppError::NoOpenVault)?;
        let ops = VaultOperations::new(&active.session).map_err(AppError::from)?;
        let vault_path =
            VaultPath::parse(path).map_err(|e| AppError::InvalidInput(e.to_string()))?;

        ops.update_file(&vault_path, content)
            .await
            .map_err(AppError::from)?;

        drop(guard);
        self.emit(AppEvent::FileUpdated {
            path: path.to_string(),
        });
        Ok(())
    }

    /// Delete a file from the vault.
    pub async fn delete_file(&self, path: &str) -> AppResult<()> {
        let guard = self.session.read().await;
        let active = guard.as_ref().ok_or(AppError::NoOpenVault)?;
        let ops = VaultOperations::new(&active.session).map_err(AppError::from)?;
        let vault_path =
            VaultPath::parse(path).map_err(|e| AppError::InvalidInput(e.to_string()))?;

        ops.delete_file(&vault_path).await.map_err(AppError::from)?;

        drop(guard);
        self.emit(AppEvent::FileDeleted {
            path: path.to_string(),
        });
        Ok(())
    }

    // -- Directory operations --

    /// Create a directory in the vault.
    pub async fn create_directory(&self, path: &str) -> AppResult<()> {
        let guard = self.session.read().await;
        let active = guard.as_ref().ok_or(AppError::NoOpenVault)?;
        let ops = VaultOperations::new(&active.session).map_err(AppError::from)?;
        let vault_path =
            VaultPath::parse(path).map_err(|e| AppError::InvalidInput(e.to_string()))?;

        ops.create_directory(&vault_path)
            .await
            .map_err(AppError::from)?;

        drop(guard);
        self.emit(AppEvent::DirectoryCreated {
            path: path.to_string(),
        });
        Ok(())
    }

    /// List directory contents.
    pub async fn list_directory(&self, path: &str) -> AppResult<Vec<DirectoryEntryDto>> {
        let guard = self.session.read().await;
        let active = guard.as_ref().ok_or(AppError::NoOpenVault)?;
        let ops = VaultOperations::new(&active.session).map_err(AppError::from)?;
        let vault_path =
            VaultPath::parse(path).map_err(|e| AppError::InvalidInput(e.to_string()))?;

        let entries = ops
            .list_directory(&vault_path)
            .await
            .map_err(AppError::from)?;

        let dtos: Vec<DirectoryEntryDto> = entries
            .into_iter()
            .map(|(name, is_directory, size)| {
                let entry_path = if path == "/" {
                    format!("/{}", name)
                } else {
                    format!("{}/{}", path.trim_end_matches('/'), name)
                };
                DirectoryEntryDto {
                    name,
                    path: entry_path,
                    is_directory,
                    size,
                    modified_at: None,
                }
            })
            .collect();

        drop(guard);
        self.emit(AppEvent::DirectoryListed {
            path: path.to_string(),
            entries: dtos.clone(),
        });
        Ok(dtos)
    }

    /// Delete an empty directory.
    pub async fn delete_directory(&self, path: &str) -> AppResult<()> {
        let guard = self.session.read().await;
        let active = guard.as_ref().ok_or(AppError::NoOpenVault)?;
        let ops = VaultOperations::new(&active.session).map_err(AppError::from)?;
        let vault_path =
            VaultPath::parse(path).map_err(|e| AppError::InvalidInput(e.to_string()))?;

        ops.delete_directory(&vault_path)
            .await
            .map_err(AppError::from)?;

        drop(guard);
        self.emit(AppEvent::DirectoryDeleted {
            path: path.to_string(),
        });
        Ok(())
    }

    /// Check if a path exists in the vault.
    pub async fn exists(&self, path: &str) -> AppResult<bool> {
        let guard = self.session.read().await;
        let active = guard.as_ref().ok_or(AppError::NoOpenVault)?;
        let ops = VaultOperations::new(&active.session).map_err(AppError::from)?;
        let vault_path =
            VaultPath::parse(path).map_err(|e| AppError::InvalidInput(e.to_string()))?;

        Ok(ops.exists(&vault_path).await)
    }

    /// Get file or directory metadata.
    pub async fn metadata(&self, path: &str) -> AppResult<FileMetadataDto> {
        let guard = self.session.read().await;
        let active = guard.as_ref().ok_or(AppError::NoOpenVault)?;
        let ops = VaultOperations::new(&active.session).map_err(AppError::from)?;
        let vault_path =
            VaultPath::parse(path).map_err(|e| AppError::InvalidInput(e.to_string()))?;

        let (name, is_directory, size) = ops.metadata(&vault_path).await.map_err(AppError::from)?;

        Ok(FileMetadataDto {
            name,
            path: path.to_string(),
            is_directory,
            size,
        })
    }

    // -- File import/export --

    /// Import a local file into the vault.
    pub async fn import_file(&self, local_path: &str, vault_path: &str) -> AppResult<()> {
        let content = tokio::fs::read(local_path)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to read local file: {}", e)))?;

        self.create_file(vault_path, &content).await
    }

    /// Export a vault file to the local filesystem.
    pub async fn export_file(&self, vault_path: &str, local_path: &str) -> AppResult<()> {
        let content = self.read_file(vault_path).await?;

        tokio::fs::write(local_path, content)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to write local file: {}", e)))?;

        Ok(())
    }

    /// Check if a vault exists at the given location.
    pub async fn vault_exists(
        &self,
        provider_type: &str,
        provider_config: serde_json::Value,
    ) -> AppResult<bool> {
        self.manager
            .vault_exists(provider_type, provider_config)
            .await
            .map_err(AppError::from)
    }
}

impl Default for AppService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_open_vault() {
        let service = AppService::new();
        let mut rx = service.subscribe();

        let result = service
            .create_vault(CreateVaultParams {
                vault_id: "test-vault".to_string(),
                password: "secure-password".to_string(),
                provider_type: "memory".to_string(),
                provider_config: serde_json::Value::Null,
            })
            .await
            .unwrap();

        assert_eq!(result.info.id, "test-vault");
        assert!(result.info.is_unlocked);
        assert_eq!(result.recovery_words.split_whitespace().count(), 24);

        // Should have received a VaultCreated event.
        let event = rx.try_recv().unwrap();
        assert!(matches!(event, AppEvent::VaultCreated(_)));
    }

    #[tokio::test]
    async fn test_file_operations() {
        let service = AppService::new();

        service
            .create_vault(CreateVaultParams {
                vault_id: "test-vault".to_string(),
                password: "password".to_string(),
                provider_type: "memory".to_string(),
                provider_config: serde_json::Value::Null,
            })
            .await
            .unwrap();

        // Create and read a file.
        service
            .create_file("/hello.txt", b"Hello, World!")
            .await
            .unwrap();

        let content = service.read_file("/hello.txt").await.unwrap();
        assert_eq!(content, b"Hello, World!");

        // Update the file.
        service
            .update_file("/hello.txt", b"Updated content")
            .await
            .unwrap();

        let content = service.read_file("/hello.txt").await.unwrap();
        assert_eq!(content, b"Updated content");

        // Delete the file.
        service.delete_file("/hello.txt").await.unwrap();
        assert!(!service.exists("/hello.txt").await.unwrap());
    }

    #[tokio::test]
    async fn test_directory_operations() {
        let service = AppService::new();

        service
            .create_vault(CreateVaultParams {
                vault_id: "test-vault".to_string(),
                password: "password".to_string(),
                provider_type: "memory".to_string(),
                provider_config: serde_json::Value::Null,
            })
            .await
            .unwrap();

        service.create_directory("/docs").await.unwrap();
        service
            .create_file("/docs/readme.txt", b"Read me")
            .await
            .unwrap();

        let entries = service.list_directory("/docs").await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "readme.txt");
        assert!(!entries[0].is_directory);
    }

    #[tokio::test]
    async fn test_lock_and_close() {
        let service = AppService::new();

        service
            .create_vault(CreateVaultParams {
                vault_id: "test-vault".to_string(),
                password: "password".to_string(),
                provider_type: "memory".to_string(),
                provider_config: serde_json::Value::Null,
            })
            .await
            .unwrap();

        assert!(service.is_vault_open().await);

        service.lock_vault().await.unwrap();

        let info = service.vault_info().await.unwrap();
        assert!(!info.is_unlocked);

        service.close_vault().await.unwrap();
        assert!(!service.is_vault_open().await);
    }

    #[tokio::test]
    async fn test_no_vault_errors() {
        let service = AppService::new();

        assert!(matches!(
            service.vault_info().await,
            Err(AppError::NoOpenVault)
        ));
        assert!(matches!(
            service.read_file("/foo").await,
            Err(AppError::NoOpenVault)
        ));
        assert!(matches!(
            service.lock_vault().await,
            Err(AppError::NoOpenVault)
        ));
    }

    #[tokio::test]
    async fn test_open_nonexistent_vault_returns_vault_not_found() {
        let service = AppService::new();

        let result = service
            .open_vault(OpenVaultParams {
                password: "password".to_string(),
                provider_type: "memory".to_string(),
                provider_config: serde_json::Value::Null,
            })
            .await;

        assert!(
            matches!(result, Err(AppError::VaultNotFound(_))),
            "expected VaultNotFound, got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_event_serialization() {
        let event = AppEvent::FileCreated {
            path: "/test.txt".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: AppEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, AppEvent::FileCreated { .. }));
    }

    #[tokio::test]
    async fn test_change_password() {
        let service = AppService::new();

        service
            .create_vault(CreateVaultParams {
                vault_id: "test-vault".to_string(),
                password: "old-password".to_string(),
                provider_type: "memory".to_string(),
                provider_config: serde_json::Value::Null,
            })
            .await
            .unwrap();

        service
            .change_password("old-password", "new-password")
            .await
            .unwrap();

        // Verify file operations still work after password change.
        service
            .create_file("/test.txt", b"test data")
            .await
            .unwrap();
        let content = service.read_file("/test.txt").await.unwrap();
        assert_eq!(content, b"test data");
    }
}
