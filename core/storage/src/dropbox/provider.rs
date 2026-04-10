//! Dropbox storage provider implementation.

use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use axiomvault_common::{Error, Result, VaultPath};

use crate::provider::{ByteStream, Metadata, StorageProvider};

use super::auth::{DropboxAuthConfig, DropboxAuthManager, DropboxTokenManager, DropboxTokens};
use super::client::{DropboxClient, DropboxMetadata};

/// Threshold for using upload sessions instead of simple upload (150 MB).
const UPLOAD_SESSION_THRESHOLD: usize = 150 * 1024 * 1024;

/// Dropbox provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropboxConfig {
    /// Root path in Dropbox (e.g., "/AxiomVault").
    pub root_path: String,
    /// OAuth2 tokens.
    pub tokens: DropboxTokens,
    /// Optional custom OAuth2 configuration.
    #[serde(default)]
    pub auth_config: Option<DropboxAuthConfig>,
}

/// Dropbox storage provider.
pub struct DropboxProvider {
    config: DropboxConfig,
    client: DropboxClient,
}

impl DropboxProvider {
    /// Create a new Dropbox provider.
    pub fn new(config: DropboxConfig) -> Result<Self> {
        let auth_config = config.auth_config.clone().unwrap_or_default();
        let auth_manager = DropboxAuthManager::new(auth_config)?;
        let token_manager = Arc::new(DropboxTokenManager::new(
            auth_manager,
            config.tokens.clone(),
        ));
        let client = DropboxClient::new(token_manager)?;

        Ok(Self { config, client })
    }

    /// Convert a VaultPath to a Dropbox API path.
    fn to_dropbox_path(&self, path: &VaultPath) -> String {
        let vault_path = path.to_string();
        if vault_path == "/" {
            self.config.root_path.clone()
        } else {
            format!("{}{}", self.config.root_path, vault_path)
        }
    }

    /// Convert DropboxMetadata to the common Metadata type.
    fn to_metadata(&self, meta: DropboxMetadata, path: &VaultPath) -> Metadata {
        Metadata {
            id: meta.id.clone(),
            name: path.name().unwrap_or("/").to_string(),
            size: meta.size,
            is_directory: meta.is_folder(),
            modified: meta.server_modified.unwrap_or_else(chrono::Utc::now),
            etag: meta.rev.clone(),
            provider_data: Some(serde_json::json!({
                "dropbox_id": meta.id,
                "path_display": meta.path_display,
                "content_hash": meta.content_hash,
            })),
        }
    }
}

#[async_trait]
impl StorageProvider for DropboxProvider {
    fn name(&self) -> &str {
        "dropbox"
    }

    async fn upload(&self, path: &VaultPath, data: Vec<u8>) -> Result<Metadata> {
        let dbx_path = self.to_dropbox_path(path);

        let meta = if data.len() > UPLOAD_SESSION_THRESHOLD {
            self.client.upload_session(&dbx_path, data).await?
        } else {
            self.client.upload(&dbx_path, data).await?
        };

        Ok(self.to_metadata(meta, path))
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
        let dbx_path = self.to_dropbox_path(path);
        self.client.download(&dbx_path).await
    }

    async fn download_stream(&self, path: &VaultPath) -> Result<ByteStream> {
        let dbx_path = self.to_dropbox_path(path);
        let byte_stream = self.client.download_stream(&dbx_path).await?;
        let stream = byte_stream.map(|result| result.map(|bytes| bytes.to_vec()));
        Ok(Box::pin(stream))
    }

    async fn exists(&self, path: &VaultPath) -> Result<bool> {
        let dbx_path = self.to_dropbox_path(path);
        match self.client.get_metadata(&dbx_path).await {
            Ok(_) => Ok(true),
            Err(Error::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn delete(&self, path: &VaultPath) -> Result<()> {
        let dbx_path = self.to_dropbox_path(path);
        self.client.delete(&dbx_path).await
    }

    async fn list(&self, path: &VaultPath) -> Result<Vec<Metadata>> {
        let dbx_path = self.to_dropbox_path(path);
        let entries = self.client.list_folder(&dbx_path).await?;

        let mut results = Vec::with_capacity(entries.len());
        for entry in entries {
            let child_path = path.join(&entry.name)?;
            results.push(self.to_metadata(entry, &child_path));
        }

        Ok(results)
    }

    async fn metadata(&self, path: &VaultPath) -> Result<Metadata> {
        let dbx_path = self.to_dropbox_path(path);
        let meta = self.client.get_metadata(&dbx_path).await?;
        Ok(self.to_metadata(meta, path))
    }

    async fn create_dir(&self, path: &VaultPath) -> Result<Metadata> {
        let dbx_path = self.to_dropbox_path(path);
        let meta = self.client.create_folder(&dbx_path).await?;
        Ok(self.to_metadata(meta, path))
    }

    async fn delete_dir(&self, path: &VaultPath) -> Result<()> {
        let dbx_path = self.to_dropbox_path(path);
        self.client.delete(&dbx_path).await
    }

    async fn rename(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        let from_path = self.to_dropbox_path(from);
        let to_path = self.to_dropbox_path(to);
        let meta = self.client.move_entry(&from_path, &to_path).await?;
        Ok(self.to_metadata(meta, to))
    }

    async fn copy(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        let from_path = self.to_dropbox_path(from);
        let to_path = self.to_dropbox_path(to);
        let meta = self.client.copy_entry(&from_path, &to_path).await?;
        Ok(self.to_metadata(meta, to))
    }
}

/// Create a Dropbox provider from configuration.
pub fn create_dropbox_provider(config: serde_json::Value) -> Result<Arc<dyn StorageProvider>> {
    let dropbox_config: DropboxConfig = serde_json::from_value(config)
        .map_err(|e| Error::InvalidInput(format!("Invalid Dropbox config: {}", e)))?;

    Ok(Arc::new(DropboxProvider::new(dropbox_config)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn create_test_config() -> DropboxConfig {
        DropboxConfig {
            root_path: "/AxiomVault".to_string(),
            tokens: DropboxTokens {
                access_token: "test_access".to_string(),
                refresh_token: "test_refresh".to_string(),
                expires_at: Utc::now() + chrono::Duration::hours(1),
            },
            auth_config: Some(DropboxAuthConfig {
                app_key: "test_key".to_string(),
                app_secret: "test_secret".to_string(),
                redirect_url: "http://localhost:8080/callback".to_string(),
            }),
        }
    }

    #[test]
    fn test_dropbox_config_serialization() {
        let config = create_test_config();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: DropboxConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.root_path, config.root_path);
    }

    #[test]
    fn test_create_provider() {
        let config = create_test_config();
        let provider = DropboxProvider::new(config).unwrap();
        assert_eq!(provider.name(), "dropbox");
    }

    #[test]
    fn test_to_dropbox_path() {
        let config = create_test_config();
        let provider = DropboxProvider::new(config).unwrap();

        let root = VaultPath::parse("/").unwrap();
        assert_eq!(provider.to_dropbox_path(&root), "/AxiomVault");

        let file = VaultPath::parse("/d/test.enc").unwrap();
        assert_eq!(provider.to_dropbox_path(&file), "/AxiomVault/d/test.enc");
    }

    #[test]
    fn test_to_metadata_file() {
        let config = create_test_config();
        let provider = DropboxProvider::new(config).unwrap();

        let dbx_meta = DropboxMetadata {
            tag: "file".to_string(),
            name: "test.txt".to_string(),
            id: "id:abc123".to_string(),
            path_lower: Some("/axiomvault/test.txt".to_string()),
            path_display: Some("/AxiomVault/test.txt".to_string()),
            size: Some(1024),
            server_modified: Some(Utc::now()),
            rev: Some("rev123".to_string()),
            content_hash: Some("hash123".to_string()),
        };

        let path = VaultPath::parse("/test.txt").unwrap();
        let meta = provider.to_metadata(dbx_meta, &path);

        assert_eq!(meta.name, "test.txt");
        assert_eq!(meta.size, Some(1024));
        assert!(!meta.is_directory);
        assert_eq!(meta.etag, Some("rev123".to_string()));
    }

    #[test]
    fn test_to_metadata_folder() {
        let config = create_test_config();
        let provider = DropboxProvider::new(config).unwrap();

        let dbx_meta = DropboxMetadata {
            tag: "folder".to_string(),
            name: "docs".to_string(),
            id: "id:folder1".to_string(),
            path_lower: Some("/axiomvault/docs".to_string()),
            path_display: Some("/AxiomVault/docs".to_string()),
            size: None,
            server_modified: None,
            rev: None,
            content_hash: None,
        };

        let path = VaultPath::parse("/docs").unwrap();
        let meta = provider.to_metadata(dbx_meta, &path);

        assert_eq!(meta.name, "docs");
        assert_eq!(meta.size, None);
        assert!(meta.is_directory);
    }

    #[test]
    fn test_create_provider_factory() {
        let config = create_test_config();
        let config_json = serde_json::to_value(config).unwrap();
        let provider = create_dropbox_provider(config_json).unwrap();
        assert_eq!(provider.name(), "dropbox");
    }

    #[test]
    fn test_create_provider_invalid_config() {
        let invalid = serde_json::json!({ "invalid": true });
        assert!(create_dropbox_provider(invalid).is_err());
    }
}
