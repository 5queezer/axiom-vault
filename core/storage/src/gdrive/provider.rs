//! Google Drive storage provider implementation.

use async_trait::async_trait;
use futures::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use axiomvault_common::{Error, Result, VaultPath};

use crate::provider::{ByteStream, Metadata, StorageProvider};

use super::auth::{AuthConfig, AuthManager, TokenManager, Tokens};
use super::client::{DriveClient, DriveFile};

/// Google Drive provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GDriveConfig {
    /// Root folder ID in Google Drive where vault is stored.
    pub folder_id: String,
    /// OAuth2 tokens.
    pub tokens: Tokens,
    /// Optional custom OAuth2 configuration.
    #[serde(default)]
    pub auth_config: Option<AuthConfig>,
}

/// Google Drive storage provider.
///
/// Implements the StorageProvider trait for Google Drive backend.
pub struct GDriveProvider {
    config: GDriveConfig,
    client: DriveClient,
    token_manager: Arc<TokenManager>,
    /// Cache of path to file ID mapping.
    path_cache: RwLock<HashMap<String, String>>,
}

impl GDriveProvider {
    /// Create a new Google Drive provider.
    ///
    /// # Preconditions
    /// - Config must have valid tokens
    /// - Root folder must exist in Google Drive
    ///
    /// # Postconditions
    /// - Provider is ready to use
    ///
    /// # Errors
    /// - Invalid configuration
    /// - Authentication errors
    pub fn new(config: GDriveConfig) -> Result<Self> {
        let auth_config = config.auth_config.clone().unwrap_or_default();

        let auth_manager = AuthManager::new(auth_config)?;
        let token_manager = Arc::new(TokenManager::new(auth_manager, config.tokens.clone()));
        let client = DriveClient::new(token_manager.clone());

        let mut path_cache = HashMap::new();
        // Cache root mapping
        path_cache.insert("/".to_string(), config.folder_id.clone());

        Ok(Self {
            config,
            client,
            token_manager,
            path_cache: RwLock::new(path_cache),
        })
    }

    /// Get current tokens (useful for persistence).
    pub async fn get_tokens(&self) -> Tokens {
        self.token_manager.get_tokens().await
    }

    /// Resolve a VaultPath to a Google Drive file ID.
    async fn resolve_path(&self, path: &VaultPath) -> Result<String> {
        let path_str = path.to_string();

        // Check cache first
        {
            let cache = self.path_cache.read().await;
            if let Some(id) = cache.get(&path_str) {
                return Ok(id.clone());
            }
        }

        // Resolve path by walking the tree
        let components = path.components();

        if components.is_empty() {
            // Root path
            return Ok(self.config.folder_id.clone());
        }

        let mut current_id = self.config.folder_id.clone();
        let mut current_path = String::from("/");

        for component in components {
            // Build current path for caching
            if current_path == "/" {
                current_path = format!("/{}", component);
            } else {
                current_path = format!("{}/{}", current_path, component);
            }

            // Check cache for this intermediate path
            {
                let cache = self.path_cache.read().await;
                if let Some(id) = cache.get(&current_path) {
                    current_id = id.clone();
                    continue;
                }
            }

            // Find the file in the current folder
            let file = self
                .client
                .find_file(component, &current_id)
                .await?
                .ok_or_else(|| {
                    Error::NotFound(format!("Path component not found: {}", component))
                })?;

            current_id = file.id.clone();

            // Cache the mapping
            let mut cache = self.path_cache.write().await;
            cache.insert(current_path.clone(), current_id.clone());
        }

        Ok(current_id)
    }

    /// Resolve parent path and return (parent_id, name).
    async fn resolve_parent(&self, path: &VaultPath) -> Result<(String, String)> {
        let parent = path
            .parent()
            .ok_or_else(|| Error::InvalidInput("Cannot get parent of root path".to_string()))?;

        let name = path
            .name()
            .ok_or_else(|| Error::InvalidInput("Path has no name component".to_string()))?
            .to_string();

        let parent_id = self.resolve_path(&parent).await?;

        Ok((parent_id, name))
    }

    /// Convert DriveFile to Metadata.
    fn to_metadata(&self, file: DriveFile, path: &VaultPath) -> Metadata {
        let file_id = file.id.clone();
        Metadata {
            id: file_id.clone(),
            name: path.name().unwrap_or("/").to_string(),
            size: file.size_bytes(),
            is_directory: file.is_folder(),
            modified: file.modified_time.unwrap_or_else(chrono::Utc::now),
            etag: file.md5_checksum.or(Some(file_id.clone())),
            provider_data: Some(serde_json::json!({
                "drive_id": file_id,
                "mime_type": file.mime_type,
                "parents": file.parents,
            })),
        }
    }

    /// Invalidate cache for a path.
    async fn invalidate_cache(&self, path: &VaultPath) {
        let path_str = path.to_string();
        let mut cache = self.path_cache.write().await;
        cache.remove(&path_str);
    }

    /// Add path to cache.
    async fn cache_path(&self, path: &VaultPath, file_id: &str) {
        let mut cache = self.path_cache.write().await;
        cache.insert(path.to_string(), file_id.to_string());
    }

    /// Internal helper for uploading data with optional resumable upload support.
    async fn upload_data(
        &self,
        path: &VaultPath,
        data: Vec<u8>,
        use_resumable_for_large: bool,
    ) -> Result<Metadata> {
        let (parent_id, name) = self.resolve_parent(path).await?;

        // Check if file already exists
        let existing = self.client.find_file(&name, &parent_id).await?;

        let file = if let Some(existing_file) = existing {
            // Update existing file
            self.client.update_file(&existing_file.id, data).await?
        } else if use_resumable_for_large && data.len() as u64 > 5 * 1024 * 1024 {
            // Use resumable upload for large files (>5MB)
            let total_size = data.len() as u64;
            let data_stream = stream::once(async { Ok(data) });
            self.client
                .upload_resumable(&name, &parent_id, Box::pin(data_stream), total_size)
                .await?
        } else {
            // Create new file
            self.client.upload_simple(&name, &parent_id, data).await?
        };

        // Cache the new path
        self.cache_path(path, &file.id).await;

        Ok(self.to_metadata(file, path))
    }
}

#[async_trait]
impl StorageProvider for GDriveProvider {
    fn name(&self) -> &str {
        "gdrive"
    }

    async fn upload(&self, path: &VaultPath, data: Vec<u8>) -> Result<Metadata> {
        self.upload_data(path, data, false).await
    }

    async fn upload_stream(&self, path: &VaultPath, stream: ByteStream) -> Result<Metadata> {
        // For streaming, we need to know the total size
        // Collect stream to determine size, then use resumable upload
        let mut data = Vec::new();
        let mut stream = stream;

        while let Some(chunk) = stream.next().await {
            data.extend_from_slice(&chunk?);
        }

        self.upload_data(path, data, true).await
    }

    async fn download(&self, path: &VaultPath) -> Result<Vec<u8>> {
        let file_id = self.resolve_path(path).await?;
        self.client.download(&file_id).await
    }

    async fn download_stream(&self, path: &VaultPath) -> Result<ByteStream> {
        let file_id = self.resolve_path(path).await?;
        let byte_stream = self.client.download_stream(&file_id).await?;

        // Convert Bytes stream to Vec<u8> stream
        let stream = byte_stream.map(|result| result.map(|bytes| bytes.to_vec()));

        Ok(Box::pin(stream))
    }

    async fn exists(&self, path: &VaultPath) -> Result<bool> {
        match self.resolve_path(path).await {
            Ok(_) => Ok(true),
            Err(Error::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn delete(&self, path: &VaultPath) -> Result<()> {
        let file_id = self.resolve_path(path).await?;
        self.client.delete(&file_id).await?;
        self.invalidate_cache(path).await;
        Ok(())
    }

    async fn list(&self, path: &VaultPath) -> Result<Vec<Metadata>> {
        let folder_id = self.resolve_path(path).await?;
        let files = self.client.list_folder(&folder_id).await?;

        let mut results = Vec::with_capacity(files.len());

        for file in files {
            let child_path = path.join(&file.name)?;

            // Cache child paths
            self.cache_path(&child_path, &file.id).await;

            results.push(self.to_metadata(file, &child_path));
        }

        Ok(results)
    }

    async fn metadata(&self, path: &VaultPath) -> Result<Metadata> {
        let file_id = self.resolve_path(path).await?;
        let file = self.client.get_file(&file_id).await?;
        Ok(self.to_metadata(file, path))
    }

    async fn create_dir(&self, path: &VaultPath) -> Result<Metadata> {
        let (parent_id, name) = self.resolve_parent(path).await?;

        // Check if already exists
        if let Some(existing) = self.client.find_file(&name, &parent_id).await? {
            if existing.is_folder() {
                return Err(Error::AlreadyExists(format!(
                    "Directory already exists: {}",
                    path
                )));
            } else {
                return Err(Error::AlreadyExists(format!(
                    "File already exists at path: {}",
                    path
                )));
            }
        }

        let folder = self.client.create_folder(&name, Some(&parent_id)).await?;
        self.cache_path(path, &folder.id).await;

        Ok(self.to_metadata(folder, path))
    }

    async fn delete_dir(&self, path: &VaultPath) -> Result<()> {
        let folder_id = self.resolve_path(path).await?;

        // Check if empty
        let contents = self.client.list_folder(&folder_id).await?;
        if !contents.is_empty() {
            return Err(Error::InvalidInput("Directory not empty".to_string()));
        }

        self.client.delete(&folder_id).await?;
        self.invalidate_cache(path).await;

        Ok(())
    }

    async fn rename(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        let from_id = self.resolve_path(from).await?;
        let from_metadata = self.client.get_file(&from_id).await?;

        let (new_parent_id, new_name) = self.resolve_parent(to).await?;

        // Check if destination already exists
        if self
            .client
            .find_file(&new_name, &new_parent_id)
            .await?
            .is_some()
        {
            return Err(Error::AlreadyExists(format!(
                "Destination already exists: {}",
                to
            )));
        }

        // Get current parent
        let current_parent = from_metadata.parents.first().cloned();

        let file = self
            .client
            .move_file(
                &from_id,
                Some(&new_name),
                Some(&new_parent_id),
                current_parent.as_deref(),
            )
            .await?;

        // Update cache
        self.invalidate_cache(from).await;
        self.cache_path(to, &file.id).await;

        Ok(self.to_metadata(file, to))
    }

    async fn copy(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        let from_id = self.resolve_path(from).await?;
        let (to_parent_id, to_name) = self.resolve_parent(to).await?;

        // Check if destination already exists
        if self
            .client
            .find_file(&to_name, &to_parent_id)
            .await?
            .is_some()
        {
            return Err(Error::AlreadyExists(format!(
                "Destination already exists: {}",
                to
            )));
        }

        let file = self
            .client
            .copy_file(&from_id, &to_name, &to_parent_id)
            .await?;
        self.cache_path(to, &file.id).await;

        Ok(self.to_metadata(file, to))
    }
}

/// Create a Google Drive provider from configuration.
pub fn create_gdrive_provider(config: serde_json::Value) -> Result<Arc<dyn StorageProvider>> {
    let gdrive_config: GDriveConfig = serde_json::from_value(config)
        .map_err(|e| Error::InvalidInput(format!("Invalid GDrive config: {}", e)))?;

    Ok(Arc::new(GDriveProvider::new(gdrive_config)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn create_test_config() -> GDriveConfig {
        GDriveConfig {
            folder_id: "test_folder_id".to_string(),
            tokens: Tokens {
                access_token: "test_access".to_string(),
                refresh_token: "test_refresh".to_string(),
                expires_at: Utc::now() + chrono::Duration::hours(1),
            },
            auth_config: Some(AuthConfig {
                client_id: "test_client".to_string(),
                client_secret: "test_secret".to_string(),
                redirect_url: "http://localhost:8080/callback".to_string(),
            }),
        }
    }

    #[test]
    fn test_gdrive_config_serialization() {
        let config = create_test_config();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: GDriveConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.folder_id, config.folder_id);
        assert_eq!(deserialized.tokens.access_token, config.tokens.access_token);
    }

    #[test]
    fn test_create_provider() {
        let config = create_test_config();
        let provider = GDriveProvider::new(config);
        assert!(provider.is_ok());

        let provider = provider.unwrap();
        assert_eq!(provider.name(), "gdrive");
    }

    #[test]
    fn test_to_metadata() {
        let config = create_test_config();
        let provider = GDriveProvider::new(config).unwrap();

        let drive_file = DriveFile {
            id: "file_id".to_string(),
            name: "test.txt".to_string(),
            mime_type: "text/plain".to_string(),
            size: Some("1024".to_string()),
            created_time: Some(Utc::now()),
            modified_time: Some(Utc::now()),
            parents: vec!["parent_id".to_string()],
            md5_checksum: Some("md5hash".to_string()),
            trashed: false,
        };

        let path = VaultPath::parse("/test.txt").unwrap();
        let metadata = provider.to_metadata(drive_file, &path);

        assert_eq!(metadata.id, "file_id");
        assert_eq!(metadata.name, "test.txt");
        assert_eq!(metadata.size, Some(1024));
        assert!(!metadata.is_directory);
        assert_eq!(metadata.etag, Some("md5hash".to_string()));
    }

    #[test]
    fn test_to_metadata_folder() {
        let config = create_test_config();
        let provider = GDriveProvider::new(config).unwrap();

        let drive_folder = DriveFile {
            id: "folder_id".to_string(),
            name: "docs".to_string(),
            mime_type: "application/vnd.google-apps.folder".to_string(),
            size: None,
            created_time: Some(Utc::now()),
            modified_time: Some(Utc::now()),
            parents: vec!["parent_id".to_string()],
            md5_checksum: None,
            trashed: false,
        };

        let path = VaultPath::parse("/docs").unwrap();
        let metadata = provider.to_metadata(drive_folder, &path);

        assert_eq!(metadata.id, "folder_id");
        assert_eq!(metadata.name, "docs");
        assert_eq!(metadata.size, None);
        assert!(metadata.is_directory);
    }

    #[test]
    fn test_create_gdrive_provider_factory() {
        let config = create_test_config();
        let config_json = serde_json::to_value(config).unwrap();

        let result = create_gdrive_provider(config_json);
        assert!(result.is_ok());

        let provider = result.unwrap();
        assert_eq!(provider.name(), "gdrive");
    }

    #[test]
    fn test_create_gdrive_provider_invalid_config() {
        let invalid_config = serde_json::json!({
            "invalid": "config"
        });

        let result = create_gdrive_provider(invalid_config);
        assert!(result.is_err());
    }
}
