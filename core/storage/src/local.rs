//! Local filesystem storage provider.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::stream;
use std::path::{Path, PathBuf};
use tokio::fs;
use uuid::Uuid;

use crate::provider::{ByteStream, Metadata, StorageProvider};
use axiomvault_common::{Error, Result, VaultPath};

/// File mode for vault files (owner read/write only).
#[cfg(unix)]
const FILE_MODE: u32 = 0o600;
/// Directory mode for vault directories (owner read/write/execute only).
#[cfg(unix)]
const DIR_MODE: u32 = 0o700;

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
    /// - On Unix, a newly created root directory is restricted to mode `0o700`
    ///
    /// # Errors
    /// - Invalid path
    /// - Permission denied
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();

        // Create root if it doesn't exist (sync for constructor).
        // On Unix, restrict mode to 0o700 so other local users cannot read
        // wrapped keys, KDF parameters, or ciphertext sitting at rest.
        if !root.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(DIR_MODE)
                    .create(&root)?;
            }
            #[cfg(not(unix))]
            {
                std::fs::create_dir_all(&root)?;
            }
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

        // Write atomically: write to a temp file in the same directory, then rename.
        // This prevents partial/corrupt files if the process is interrupted mid-write.
        let parent_dir = fs_path
            .parent()
            .ok_or_else(|| Error::InvalidInput("Cannot write to root path".to_string()))?;
        let tmp_path = parent_dir.join(format!(
            ".{}.tmp.{}",
            fs_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file"),
            uuid::Uuid::new_v4()
        ));

        // Create the temp file with restrictive permissions on Unix so
        // other local users cannot read ciphertext or vault config that
        // contains wrapped keys / KDF parameters. On non-Unix targets we
        // fall back to the default permissions (the file will inherit
        // platform defaults).
        #[cfg(unix)]
        {
            use tokio::io::AsyncWriteExt;

            // tokio::fs::OpenOptions exposes `mode()` directly on Unix
            // (cfg-gated to the unix targets), so no extension trait
            // import is required.
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(FILE_MODE)
                .open(&tmp_path)
                .await?;
            if let Err(e) = file.write_all(&data).await {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(e.into());
            }
            if let Err(e) = file.sync_all().await {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(e.into());
            }
        }
        #[cfg(not(unix))]
        {
            fs::write(&tmp_path, &data).await?;
        }

        if let Err(e) = fs::rename(&tmp_path, &fs_path).await {
            // Best-effort cleanup; ignore secondary error
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e.into());
        }

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

        // Restrict directory mode so other local users cannot list its
        // contents (defence-in-depth around vault metadata layout).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&fs_path, std::fs::Permissions::from_mode(DIR_MODE)).await?;
        }

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
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&to_path, std::fs::Permissions::from_mode(DIR_MODE)).await?;
            }
        } else {
            fs::copy(&from_path, &to_path).await?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&to_path, std::fs::Permissions::from_mode(FILE_MODE)).await?;
            }
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

    #[cfg(unix)]
    #[tokio::test]
    async fn test_local_upload_sets_restrictive_file_mode() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let provider = LocalProvider::new(temp.path()).unwrap();
        let path = VaultPath::parse("/secret.bin").unwrap();
        provider
            .upload(&path, b"ciphertext".to_vec())
            .await
            .unwrap();

        let fs_path = temp.path().join("secret.bin");
        let mode = std::fs::metadata(&fs_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "uploaded file must be owner-only readable");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_local_create_dir_sets_restrictive_dir_mode() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let provider = LocalProvider::new(temp.path()).unwrap();
        let path = VaultPath::parse("/private").unwrap();
        provider.create_dir(&path).await.unwrap();

        let fs_path = temp.path().join("private");
        let mode = std::fs::metadata(&fs_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "created directory must be owner-only");
    }

    #[cfg(unix)]
    #[test]
    fn test_local_new_creates_root_with_restrictive_dir_mode() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let root = temp.path().join("vault-root");
        assert!(!root.exists());
        LocalProvider::new(&root).unwrap();

        let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "newly created vault root must be owner-only");
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
