//! Local filesystem storage provider.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::stream;
use std::path::{Path, PathBuf};
use tokio::fs;
use uuid::Uuid;

use crate::provider::{ByteStream, Metadata, StorageProvider};
use axiomvault_common::{Error, Result, VaultPath};

/// Local filesystem storage provider.
///
/// Stores vault data in a local directory structure.
pub struct LocalProvider {
    root: PathBuf,
}

impl LocalProvider {
    /// Create a new local provider with the given root directory.
    ///
    /// # Preconditions
    /// - Root path must be a valid directory path
    ///
    /// # Postconditions
    /// - Provider is ready to use
    /// - Root directory is created if it doesn't exist
    ///
    /// # Errors
    /// - Invalid path
    /// - Permission denied
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();

        // Create root if it doesn't exist (sync for constructor)
        if !root.exists() {
            std::fs::create_dir_all(&root)?;
        }

        Ok(Self { root })
    }

    /// Convert a VaultPath to a filesystem path.
    fn to_fs_path(&self, path: &VaultPath) -> PathBuf {
        let mut fs_path = self.root.clone();
        for component in path.components() {
            fs_path.push(component);
        }
        fs_path
    }

    /// Create metadata from filesystem metadata.
    fn create_metadata(&self, path: &VaultPath, fs_meta: std::fs::Metadata) -> Metadata {
        let modified: DateTime<Utc> = fs_meta
            .modified()
            .map(|t| t.into())
            .unwrap_or_else(|_| Utc::now());

        Metadata {
            id: Uuid::new_v4().to_string(),
            name: path.name().unwrap_or("/").to_string(),
            size: if fs_meta.is_file() {
                Some(fs_meta.len())
            } else {
                None
            },
            is_directory: fs_meta.is_dir(),
            modified,
            etag: Some(format!("{}-{}", modified.timestamp(), fs_meta.len())),
            provider_data: None,
        }
    }
}

#[async_trait]
impl StorageProvider for LocalProvider {
    fn name(&self) -> &str {
        "local"
    }

    async fn upload(&self, path: &VaultPath, data: Vec<u8>) -> Result<Metadata> {
        let fs_path = self.to_fs_path(path);

        // Check parent exists
        if let Some(parent) = fs_path.parent() {
            if !parent.exists() {
                return Err(Error::NotFound("Parent directory not found".to_string()));
            }
        }

        fs::write(&fs_path, &data).await?;

        let fs_meta = fs::metadata(&fs_path).await?;
        Ok(self.create_metadata(path, fs_meta))
    }

    async fn upload_stream(&self, path: &VaultPath, mut stream: ByteStream) -> Result<Metadata> {
        use futures::StreamExt;
        let mut data = Vec::new();

        while let Some(chunk) = stream.next().await {
            data.extend_from_slice(&chunk?);
        }

        self.upload(path, data).await
    }

    async fn download(&self, path: &VaultPath) -> Result<Vec<u8>> {
        let fs_path = self.to_fs_path(path);

        if !fs_path.exists() {
            return Err(Error::NotFound(format!("File not found: {}", path)));
        }

        if fs_path.is_dir() {
            return Err(Error::InvalidInput("Cannot download directory".to_string()));
        }

        Ok(fs::read(&fs_path).await?)
    }

    async fn download_stream(&self, path: &VaultPath) -> Result<ByteStream> {
        let data = self.download(path).await?;
        let stream = stream::once(async move { Ok(data) });
        Ok(Box::pin(stream))
    }

    async fn exists(&self, path: &VaultPath) -> Result<bool> {
        let fs_path = self.to_fs_path(path);
        Ok(fs_path.exists())
    }

    async fn delete(&self, path: &VaultPath) -> Result<()> {
        let fs_path = self.to_fs_path(path);

        if !fs_path.exists() {
            return Err(Error::NotFound(format!("File not found: {}", path)));
        }

        if fs_path.is_dir() {
            return Err(Error::InvalidInput(
                "Use delete_dir for directories".to_string(),
            ));
        }

        fs::remove_file(&fs_path).await?;
        Ok(())
    }

    async fn list(&self, path: &VaultPath) -> Result<Vec<Metadata>> {
        let fs_path = self.to_fs_path(path);

        if !fs_path.exists() {
            return Err(Error::NotFound(format!("Directory not found: {}", path)));
        }

        if !fs_path.is_dir() {
            return Err(Error::InvalidInput("Not a directory".to_string()));
        }

        let mut results = Vec::new();
        let mut entries = fs::read_dir(&fs_path).await?;

        while let Some(entry) = entries.next_entry().await? {
            let entry_path = entry.path();
            let name = entry_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            let child_vault_path = path.join(&name)?;
            let fs_meta = entry.metadata().await?;
            results.push(self.create_metadata(&child_vault_path, fs_meta));
        }

        Ok(results)
    }

    async fn metadata(&self, path: &VaultPath) -> Result<Metadata> {
        let fs_path = self.to_fs_path(path);

        if !fs_path.exists() {
            return Err(Error::NotFound(format!("Path not found: {}", path)));
        }

        let fs_meta = fs::metadata(&fs_path).await?;
        Ok(self.create_metadata(path, fs_meta))
    }

    async fn create_dir(&self, path: &VaultPath) -> Result<Metadata> {
        let fs_path = self.to_fs_path(path);

        if fs_path.exists() {
            return Err(Error::AlreadyExists(format!(
                "Path already exists: {}",
                path
            )));
        }

        fs::create_dir(&fs_path).await?;

        let fs_meta = fs::metadata(&fs_path).await?;
        Ok(self.create_metadata(path, fs_meta))
    }

    async fn delete_dir(&self, path: &VaultPath) -> Result<()> {
        let fs_path = self.to_fs_path(path);

        if !fs_path.exists() {
            return Err(Error::NotFound(format!("Directory not found: {}", path)));
        }

        if !fs_path.is_dir() {
            return Err(Error::InvalidInput("Not a directory".to_string()));
        }

        // Check if empty
        let mut entries = fs::read_dir(&fs_path).await?;
        if entries.next_entry().await?.is_some() {
            return Err(Error::InvalidInput("Directory not empty".to_string()));
        }

        fs::remove_dir(&fs_path).await?;
        Ok(())
    }

    async fn rename(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        let from_path = self.to_fs_path(from);
        let to_path = self.to_fs_path(to);

        if !from_path.exists() {
            return Err(Error::NotFound(format!("Source not found: {}", from)));
        }

        if to_path.exists() {
            return Err(Error::AlreadyExists(format!(
                "Destination already exists: {}",
                to
            )));
        }

        fs::rename(&from_path, &to_path).await?;

        let fs_meta = fs::metadata(&to_path).await?;
        Ok(self.create_metadata(to, fs_meta))
    }

    async fn copy(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        let from_path = self.to_fs_path(from);
        let to_path = self.to_fs_path(to);

        if !from_path.exists() {
            return Err(Error::NotFound(format!("Source not found: {}", from)));
        }

        if to_path.exists() {
            return Err(Error::AlreadyExists(format!(
                "Destination already exists: {}",
                to
            )));
        }

        if from_path.is_dir() {
            fs::create_dir(&to_path).await?;
        } else {
            fs::copy(&from_path, &to_path).await?;
        }

        let fs_meta = fs::metadata(&to_path).await?;
        Ok(self.create_metadata(to, fs_meta))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_local_upload_download() {
        let temp = TempDir::new().unwrap();
        let provider = LocalProvider::new(temp.path()).unwrap();
        let path = VaultPath::parse("/test.txt").unwrap();
        let data = b"Hello, Local!".to_vec();

        provider.upload(&path, data.clone()).await.unwrap();
        let downloaded = provider.download(&path).await.unwrap();

        assert_eq!(downloaded, data);
    }

    #[tokio::test]
    async fn test_local_create_dir() {
        let temp = TempDir::new().unwrap();
        let provider = LocalProvider::new(temp.path()).unwrap();
        let path = VaultPath::parse("/mydir").unwrap();

        let metadata = provider.create_dir(&path).await.unwrap();
        assert!(metadata.is_directory);
    }

    #[tokio::test]
    async fn test_local_list() {
        let temp = TempDir::new().unwrap();
        let provider = LocalProvider::new(temp.path()).unwrap();

        provider
            .create_dir(&VaultPath::parse("/dir").unwrap())
            .await
            .unwrap();
        provider
            .upload(&VaultPath::parse("/dir/file1.txt").unwrap(), vec![1])
            .await
            .unwrap();
        provider
            .upload(&VaultPath::parse("/dir/file2.txt").unwrap(), vec![2])
            .await
            .unwrap();

        let contents = provider
            .list(&VaultPath::parse("/dir").unwrap())
            .await
            .unwrap();
        assert_eq!(contents.len(), 2);
    }
}
