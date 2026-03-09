//! iCloud Drive storage provider implementation.
//!
//! Wraps `LocalProvider` around the iCloud Drive mount point on macOS.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use axiomvault_common::{Error, Result, VaultPath};

use crate::local::LocalProvider;
use crate::provider::{ByteStream, Metadata, StorageProvider};

/// iCloud Drive provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ICloudConfig {
    /// Override the auto-detected iCloud Drive path.
    /// If not set, the provider auto-detects on macOS.
    #[serde(default)]
    pub root_path: Option<String>,
    /// Subfolder within iCloud Drive (e.g., "AxiomVault").
    #[serde(default)]
    pub subfolder: Option<String>,
}

/// iCloud Drive storage provider.
///
/// Delegates all operations to an inner `LocalProvider` pointed at the
/// iCloud Drive folder on macOS. Syncing is handled transparently by the OS.
pub struct ICloudProvider {
    local: LocalProvider,
}

impl ICloudProvider {
    /// Create a new iCloud Drive provider.
    ///
    /// Auto-detects the iCloud Drive folder on macOS, or uses the
    /// configured `root_path` override.
    pub fn new(config: ICloudConfig) -> Result<Self> {
        let base_path = match config.root_path {
            Some(ref path) => std::path::PathBuf::from(path),
            None => super::detect_icloud_path().ok_or_else(|| {
                Error::NotFound(
                    "iCloud Drive not found. \
                     iCloud Drive is only available on macOS with iCloud enabled. \
                     You can set a custom path via the 'root_path' config option."
                        .to_string(),
                )
            })?,
        };

        let root = match config.subfolder {
            Some(ref sub) => base_path.join(sub),
            None => base_path,
        };

        let local = LocalProvider::new(&root)?;
        Ok(Self { local })
    }
}

#[async_trait]
impl StorageProvider for ICloudProvider {
    fn name(&self) -> &str {
        "icloud"
    }

    async fn upload(&self, path: &VaultPath, data: Vec<u8>) -> Result<Metadata> {
        self.local.upload(path, data).await
    }

    async fn upload_stream(&self, path: &VaultPath, stream: ByteStream) -> Result<Metadata> {
        self.local.upload_stream(path, stream).await
    }

    async fn download(&self, path: &VaultPath) -> Result<Vec<u8>> {
        self.local.download(path).await
    }

    async fn download_stream(&self, path: &VaultPath) -> Result<ByteStream> {
        self.local.download_stream(path).await
    }

    async fn exists(&self, path: &VaultPath) -> Result<bool> {
        self.local.exists(path).await
    }

    async fn delete(&self, path: &VaultPath) -> Result<()> {
        self.local.delete(path).await
    }

    async fn list(&self, path: &VaultPath) -> Result<Vec<Metadata>> {
        self.local.list(path).await
    }

    async fn metadata(&self, path: &VaultPath) -> Result<Metadata> {
        self.local.metadata(path).await
    }

    async fn create_dir(&self, path: &VaultPath) -> Result<Metadata> {
        self.local.create_dir(path).await
    }

    async fn delete_dir(&self, path: &VaultPath) -> Result<()> {
        self.local.delete_dir(path).await
    }

    async fn rename(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        self.local.rename(from, to).await
    }

    async fn copy(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        self.local.copy(from, to).await
    }
}

/// Create an iCloud Drive provider from configuration.
pub fn create_icloud_provider(config: serde_json::Value) -> Result<Arc<dyn StorageProvider>> {
    let icloud_config: ICloudConfig = serde_json::from_value(config)
        .map_err(|e| Error::InvalidInput(format!("Invalid iCloud config: {}", e)))?;

    Ok(Arc::new(ICloudProvider::new(icloud_config)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_icloud_config_serialization() {
        let config = ICloudConfig {
            root_path: Some("/tmp/test-icloud".to_string()),
            subfolder: Some("AxiomVault".to_string()),
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: ICloudConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.root_path, config.root_path);
        assert_eq!(deserialized.subfolder, config.subfolder);
    }

    #[test]
    fn test_create_provider_with_custom_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = ICloudConfig {
            root_path: Some(dir.path().to_string_lossy().to_string()),
            subfolder: None,
        };

        let provider = ICloudProvider::new(config).unwrap();
        assert_eq!(provider.name(), "icloud");
    }

    #[test]
    fn test_create_provider_factory() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = serde_json::json!({
            "root_path": dir.path().to_string_lossy().to_string()
        });

        let provider = create_icloud_provider(config).unwrap();
        assert_eq!(provider.name(), "icloud");
    }

    #[tokio::test]
    async fn test_icloud_basic_operations() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = ICloudConfig {
            root_path: Some(dir.path().to_string_lossy().to_string()),
            subfolder: None,
        };

        let provider = ICloudProvider::new(config).unwrap();

        // Create a directory
        let dir_path = VaultPath::parse("test-dir").unwrap();
        provider.create_dir(&dir_path).await.unwrap();
        assert!(provider.exists(&dir_path).await.unwrap());

        // Upload a file
        let file_path = VaultPath::parse("test-dir/hello.txt").unwrap();
        provider
            .upload(&file_path, b"hello world".to_vec())
            .await
            .unwrap();

        // Download and verify
        let data = provider.download(&file_path).await.unwrap();
        assert_eq!(data, b"hello world");

        // List directory
        let entries = provider.list(&dir_path).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "hello.txt");

        // Delete
        provider.delete(&file_path).await.unwrap();
        assert!(!provider.exists(&file_path).await.unwrap());
    }
}
