//! OneDrive storage provider implementation.

use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use axiomvault_common::{Error, Result, VaultPath};

use crate::provider::{ByteStream, Metadata, StorageProvider};

use super::auth::{OneDriveAuthConfig, OneDriveAuthManager, OneDriveTokenManager, OneDriveTokens};
use super::client::{DriveItem, OneDriveClient};

/// Threshold for using upload sessions instead of simple upload (4 MB).
const UPLOAD_SESSION_THRESHOLD: usize = 4 * 1024 * 1024;

/// OneDrive provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OneDriveConfig {
    /// Root path in OneDrive (e.g., "/AxiomVault").
    pub root_path: String,
    /// OAuth2 tokens.
    pub tokens: OneDriveTokens,
    /// Optional custom OAuth2 configuration.
    #[serde(default)]
    pub auth_config: Option<OneDriveAuthConfig>,
}

/// OneDrive storage provider.
pub struct OneDriveProvider {
    config: OneDriveConfig,
    client: OneDriveClient,
}

impl OneDriveProvider {
    /// Create a new OneDrive provider.
    pub fn new(config: OneDriveConfig) -> Result<Self> {
        let auth_config = config.auth_config.clone().unwrap_or_default();
        let auth_manager = OneDriveAuthManager::new(auth_config)?;
        let token_manager = Arc::new(OneDriveTokenManager::new(
            auth_manager,
            config.tokens.clone(),
        ));
        let client = OneDriveClient::new(token_manager);

        Ok(Self { config, client })
    }

    /// Convert a VaultPath to a OneDrive API path.
    fn to_onedrive_path(&self, path: &VaultPath) -> String {
        let vault_path = path.to_string();
        if vault_path == "/" {
            self.config.root_path.clone()
        } else {
            format!("{}{}", self.config.root_path, vault_path)
        }
    }

    /// Get the parent path and name from a full path.
    fn parent_and_name(path: &str) -> Option<(&str, &str)> {
        let path = path.trim_end_matches('/');
        path.rsplit_once('/')
    }

    /// Convert DriveItem to the common Metadata type.
    fn to_metadata(&self, item: DriveItem, path: &VaultPath) -> Metadata {
        Metadata {
            id: item.id.clone(),
            name: path.name().unwrap_or("/").to_string(),
            size: item.size,
            is_directory: item.is_folder(),
            modified: item
                .last_modified_date_time
                .unwrap_or_else(chrono::Utc::now),
            etag: item.etag.clone(),
            provider_data: Some(serde_json::json!({
                "onedrive_id": item.id,
                "etag": item.etag,
            })),
        }
    }
}

#[async_trait]
impl StorageProvider for OneDriveProvider {
    fn name(&self) -> &str {
        "onedrive"
    }

    async fn upload(&self, path: &VaultPath, data: Vec<u8>) -> Result<Metadata> {
        let od_path = self.to_onedrive_path(path);

        let item = if data.len() > UPLOAD_SESSION_THRESHOLD {
            self.client.upload_session(&od_path, data).await?
        } else {
            self.client.upload(&od_path, data).await?
        };

        Ok(self.to_metadata(item, path))
    }

    async fn upload_stream(&self, path: &VaultPath, stream: ByteStream) -> Result<Metadata> {
        let mut data = Vec::new();
        let mut stream = stream;
        while let Some(chunk) = stream.next().await {
            data.extend_from_slice(&chunk?);
        }
        self.upload(path, data).await
    }

    async fn download(&self, path: &VaultPath) -> Result<Vec<u8>> {
        let od_path = self.to_onedrive_path(path);
        self.client.download(&od_path).await
    }

    async fn download_stream(&self, path: &VaultPath) -> Result<ByteStream> {
        let od_path = self.to_onedrive_path(path);
        let byte_stream = self.client.download_stream(&od_path).await?;
        let stream = byte_stream.map(|result| result.map(|bytes| bytes.to_vec()));
        Ok(Box::pin(stream))
    }

    async fn exists(&self, path: &VaultPath) -> Result<bool> {
        let od_path = self.to_onedrive_path(path);
        match self.client.get_metadata(&od_path).await {
            Ok(_) => Ok(true),
            Err(Error::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn delete(&self, path: &VaultPath) -> Result<()> {
        let od_path = self.to_onedrive_path(path);
        self.client.delete(&od_path).await
    }

    async fn list(&self, path: &VaultPath) -> Result<Vec<Metadata>> {
        let od_path = self.to_onedrive_path(path);
        let items = self.client.list_children(&od_path).await?;

        let mut results = Vec::with_capacity(items.len());
        for item in items {
            let child_path = path.join(&item.name)?;
            results.push(self.to_metadata(item, &child_path));
        }

        Ok(results)
    }

    async fn metadata(&self, path: &VaultPath) -> Result<Metadata> {
        let od_path = self.to_onedrive_path(path);
        let item = self.client.get_metadata(&od_path).await?;
        Ok(self.to_metadata(item, path))
    }

    async fn create_dir(&self, path: &VaultPath) -> Result<Metadata> {
        let od_path = self.to_onedrive_path(path);
        let (parent, name) = Self::parent_and_name(&od_path)
            .ok_or_else(|| Error::InvalidInput("Cannot create root directory".to_string()))?;
        let item = self.client.create_folder(parent, name).await?;
        Ok(self.to_metadata(item, path))
    }

    async fn delete_dir(&self, path: &VaultPath) -> Result<()> {
        let od_path = self.to_onedrive_path(path);
        self.client.delete(&od_path).await
    }

    async fn rename(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        let from_path = self.to_onedrive_path(from);
        let to_path = self.to_onedrive_path(to);
        let (to_parent, to_name) = Self::parent_and_name(&to_path)
            .ok_or_else(|| Error::InvalidInput("Cannot rename to root".to_string()))?;
        let item = self
            .client
            .move_item(&from_path, to_parent, to_name)
            .await?;
        Ok(self.to_metadata(item, to))
    }

    async fn copy(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        let from_path = self.to_onedrive_path(from);
        let to_path = self.to_onedrive_path(to);
        let (to_parent, to_name) = Self::parent_and_name(&to_path)
            .ok_or_else(|| Error::InvalidInput("Cannot copy to root".to_string()))?;
        let item = self
            .client
            .copy_item(&from_path, to_parent, to_name)
            .await?;
        Ok(self.to_metadata(item, to))
    }
}

/// Create a OneDrive provider from configuration.
pub fn create_onedrive_provider(config: serde_json::Value) -> Result<Arc<dyn StorageProvider>> {
    let onedrive_config: OneDriveConfig = serde_json::from_value(config)
        .map_err(|e| Error::InvalidInput(format!("Invalid OneDrive config: {}", e)))?;

    Ok(Arc::new(OneDriveProvider::new(onedrive_config)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn create_test_config() -> OneDriveConfig {
        OneDriveConfig {
            root_path: "/AxiomVault".to_string(),
            tokens: OneDriveTokens {
                access_token: "test_access".to_string(),
                refresh_token: "test_refresh".to_string(),
                expires_at: Utc::now() + chrono::Duration::hours(1),
            },
            auth_config: Some(OneDriveAuthConfig {
                client_id: "test_id".to_string(),
                client_secret: "test_secret".to_string(),
                redirect_url: "http://localhost:8080/callback".to_string(),
            }),
        }
    }

    #[test]
    fn test_onedrive_config_serialization() {
        let config = create_test_config();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: OneDriveConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.root_path, config.root_path);
    }

    #[test]
    fn test_create_provider() {
        let config = create_test_config();
        let provider = OneDriveProvider::new(config).unwrap();
        assert_eq!(provider.name(), "onedrive");
    }

    #[test]
    fn test_to_onedrive_path() {
        let config = create_test_config();
        let provider = OneDriveProvider::new(config).unwrap();

        let root = VaultPath::parse("/").unwrap();
        assert_eq!(provider.to_onedrive_path(&root), "/AxiomVault");

        let file = VaultPath::parse("/d/test.enc").unwrap();
        assert_eq!(provider.to_onedrive_path(&file), "/AxiomVault/d/test.enc");
    }

    #[test]
    fn test_to_metadata_file() {
        let config = create_test_config();
        let provider = OneDriveProvider::new(config).unwrap();

        let item = DriveItem {
            id: "item123".to_string(),
            name: "test.txt".to_string(),
            size: Some(1024),
            last_modified_date_time: Some(Utc::now()),
            etag: Some("etag123".to_string()),
            file: Some(super::super::client::FileFacet {
                mime_type: Some("text/plain".to_string()),
                hashes: None,
            }),
            folder: None,
            parent_reference: None,
        };

        let path = VaultPath::parse("/test.txt").unwrap();
        let meta = provider.to_metadata(item, &path);

        assert_eq!(meta.name, "test.txt");
        assert_eq!(meta.size, Some(1024));
        assert!(!meta.is_directory);
        assert_eq!(meta.etag, Some("etag123".to_string()));
    }

    #[test]
    fn test_to_metadata_folder() {
        let config = create_test_config();
        let provider = OneDriveProvider::new(config).unwrap();

        let item = DriveItem {
            id: "folder1".to_string(),
            name: "docs".to_string(),
            size: None,
            last_modified_date_time: None,
            etag: None,
            file: None,
            folder: Some(super::super::client::FolderFacet { child_count: 5 }),
            parent_reference: None,
        };

        let path = VaultPath::parse("/docs").unwrap();
        let meta = provider.to_metadata(item, &path);

        assert_eq!(meta.name, "docs");
        assert_eq!(meta.size, None);
        assert!(meta.is_directory);
    }

    #[test]
    fn test_parent_and_name() {
        assert_eq!(
            OneDriveProvider::parent_and_name("/AxiomVault/test.txt"),
            Some(("/AxiomVault", "test.txt"))
        );
        assert_eq!(
            OneDriveProvider::parent_and_name("/AxiomVault/d/test.enc"),
            Some(("/AxiomVault/d", "test.enc"))
        );
    }

    #[test]
    fn test_create_provider_factory() {
        let config = create_test_config();
        let config_json = serde_json::to_value(config).unwrap();
        let provider = create_onedrive_provider(config_json).unwrap();
        assert_eq!(provider.name(), "onedrive");
    }

    #[test]
    fn test_create_provider_invalid_config() {
        let invalid = serde_json::json!({ "invalid": true });
        assert!(create_onedrive_provider(invalid).is_err());
    }
}
