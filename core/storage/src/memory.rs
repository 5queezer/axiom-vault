//! In-memory storage provider for testing.

use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use futures::stream;
use uuid::Uuid;

use axiomvault_common::{Error, Result, VaultPath};
use crate::provider::{ByteStream, Metadata, StorageProvider};

/// In-memory storage entry.
#[derive(Debug, Clone)]
enum Entry {
    File {
        data: Vec<u8>,
        metadata: Metadata,
    },
    Directory {
        metadata: Metadata,
    },
}

/// In-memory storage provider.
///
/// Useful for testing and development. All data is stored in memory
/// and lost on drop.
pub struct MemoryProvider {
    storage: Arc<RwLock<HashMap<String, Entry>>>,
}

impl MemoryProvider {
    /// Create a new empty memory provider.
    pub fn new() -> Self {
        let storage = Arc::new(RwLock::new(HashMap::new()));

        // Create root directory
        let root_meta = Metadata {
            id: Uuid::new_v4().to_string(),
            name: "/".to_string(),
            size: None,
            is_directory: true,
            modified: Utc::now(),
            etag: Some(Uuid::new_v4().to_string()),
            provider_data: None,
        };

        storage
            .write()
            .unwrap()
            .insert("/".to_string(), Entry::Directory { metadata: root_meta });

        Self { storage }
    }

    fn path_to_key(path: &VaultPath) -> String {
        path.to_string_path()
    }
}

impl Default for MemoryProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StorageProvider for MemoryProvider {
    fn name(&self) -> &str {
        "memory"
    }

    async fn upload(&self, path: &VaultPath, data: Vec<u8>) -> Result<Metadata> {
        let key = Self::path_to_key(path);

        // Check parent exists
        if let Some(parent) = path.parent() {
            let parent_key = Self::path_to_key(&parent);
            let storage = self.storage.read().unwrap();
            match storage.get(&parent_key) {
                Some(Entry::Directory { .. }) => {}
                Some(Entry::File { .. }) => {
                    return Err(Error::InvalidInput("Parent is a file".to_string()));
                }
                None => {
                    return Err(Error::NotFound("Parent directory not found".to_string()));
                }
            }
        }

        let metadata = Metadata {
            id: Uuid::new_v4().to_string(),
            name: path.name().unwrap_or("/").to_string(),
            size: Some(data.len() as u64),
            is_directory: false,
            modified: Utc::now(),
            etag: Some(Uuid::new_v4().to_string()),
            provider_data: None,
        };

        let entry = Entry::File {
            data,
            metadata: metadata.clone(),
        };

        self.storage.write().unwrap().insert(key, entry);

        Ok(metadata)
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
        let key = Self::path_to_key(path);
        let storage = self.storage.read().unwrap();

        match storage.get(&key) {
            Some(Entry::File { data, .. }) => Ok(data.clone()),
            Some(Entry::Directory { .. }) => {
                Err(Error::InvalidInput("Cannot download directory".to_string()))
            }
            None => Err(Error::NotFound(format!("File not found: {}", path))),
        }
    }

    async fn download_stream(&self, path: &VaultPath) -> Result<ByteStream> {
        let data = self.download(path).await?;
        let stream = stream::once(async move { Ok(data) });
        Ok(Box::pin(stream))
    }

    async fn exists(&self, path: &VaultPath) -> Result<bool> {
        let key = Self::path_to_key(path);
        Ok(self.storage.read().unwrap().contains_key(&key))
    }

    async fn delete(&self, path: &VaultPath) -> Result<()> {
        let key = Self::path_to_key(path);
        let mut storage = self.storage.write().unwrap();

        match storage.get(&key) {
            Some(Entry::File { .. }) => {
                storage.remove(&key);
                Ok(())
            }
            Some(Entry::Directory { .. }) => {
                Err(Error::InvalidInput("Use delete_dir for directories".to_string()))
            }
            None => Err(Error::NotFound(format!("File not found: {}", path))),
        }
    }

    async fn list(&self, path: &VaultPath) -> Result<Vec<Metadata>> {
        let key = Self::path_to_key(path);
        let storage = self.storage.read().unwrap();

        // Verify path is a directory
        match storage.get(&key) {
            Some(Entry::Directory { .. }) => {}
            Some(Entry::File { .. }) => {
                return Err(Error::InvalidInput("Not a directory".to_string()));
            }
            None => {
                return Err(Error::NotFound(format!("Directory not found: {}", path)));
            }
        }

        let prefix = if path.is_root() {
            "/".to_string()
        } else {
            format!("{}/", key)
        };

        let mut results = Vec::new();
        for (entry_key, entry) in storage.iter() {
            if entry_key == &key {
                continue; // Skip self
            }

            // Check if this entry is a direct child
            if entry_key.starts_with(&prefix) {
                let relative = &entry_key[prefix.len()..];
                // Only include direct children (no more slashes)
                if !relative.contains('/') {
                    let meta = match entry {
                        Entry::File { metadata, .. } => metadata.clone(),
                        Entry::Directory { metadata } => metadata.clone(),
                    };
                    results.push(meta);
                }
            }
        }

        Ok(results)
    }

    async fn metadata(&self, path: &VaultPath) -> Result<Metadata> {
        let key = Self::path_to_key(path);
        let storage = self.storage.read().unwrap();

        match storage.get(&key) {
            Some(Entry::File { metadata, .. }) => Ok(metadata.clone()),
            Some(Entry::Directory { metadata }) => Ok(metadata.clone()),
            None => Err(Error::NotFound(format!("Path not found: {}", path))),
        }
    }

    async fn create_dir(&self, path: &VaultPath) -> Result<Metadata> {
        let key = Self::path_to_key(path);

        // Check parent exists
        if let Some(parent) = path.parent() {
            let parent_key = Self::path_to_key(&parent);
            let storage = self.storage.read().unwrap();
            match storage.get(&parent_key) {
                Some(Entry::Directory { .. }) => {}
                Some(Entry::File { .. }) => {
                    return Err(Error::InvalidInput("Parent is a file".to_string()));
                }
                None => {
                    return Err(Error::NotFound("Parent directory not found".to_string()));
                }
            }
        }

        let mut storage = self.storage.write().unwrap();

        // Check if already exists
        if storage.contains_key(&key) {
            return Err(Error::AlreadyExists(format!(
                "Path already exists: {}",
                path
            )));
        }

        let metadata = Metadata {
            id: Uuid::new_v4().to_string(),
            name: path.name().unwrap_or("/").to_string(),
            size: None,
            is_directory: true,
            modified: Utc::now(),
            etag: Some(Uuid::new_v4().to_string()),
            provider_data: None,
        };

        storage.insert(key, Entry::Directory { metadata: metadata.clone() });

        Ok(metadata)
    }

    async fn delete_dir(&self, path: &VaultPath) -> Result<()> {
        let key = Self::path_to_key(path);

        // Check if directory is empty
        let contents = self.list(path).await?;
        if !contents.is_empty() {
            return Err(Error::InvalidInput("Directory not empty".to_string()));
        }

        let mut storage = self.storage.write().unwrap();
        match storage.get(&key) {
            Some(Entry::Directory { .. }) => {
                storage.remove(&key);
                Ok(())
            }
            Some(Entry::File { .. }) => {
                Err(Error::InvalidInput("Not a directory".to_string()))
            }
            None => Err(Error::NotFound(format!("Directory not found: {}", path))),
        }
    }

    async fn rename(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        let from_key = Self::path_to_key(from);
        let to_key = Self::path_to_key(to);

        let mut storage = self.storage.write().unwrap();

        if storage.contains_key(&to_key) {
            return Err(Error::AlreadyExists(format!(
                "Destination already exists: {}",
                to
            )));
        }

        let entry = storage
            .remove(&from_key)
            .ok_or_else(|| Error::NotFound(format!("Source not found: {}", from)))?;

        let new_entry = match entry {
            Entry::File { data, mut metadata } => {
                metadata.name = to.name().unwrap_or("/").to_string();
                metadata.modified = Utc::now();
                metadata.etag = Some(Uuid::new_v4().to_string());
                Entry::File { data, metadata }
            }
            Entry::Directory { mut metadata } => {
                metadata.name = to.name().unwrap_or("/").to_string();
                metadata.modified = Utc::now();
                metadata.etag = Some(Uuid::new_v4().to_string());
                Entry::Directory { metadata }
            }
        };

        let result_metadata = match &new_entry {
            Entry::File { metadata, .. } => metadata.clone(),
            Entry::Directory { metadata } => metadata.clone(),
        };

        storage.insert(to_key, new_entry);

        Ok(result_metadata)
    }

    async fn copy(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        let from_key = Self::path_to_key(from);
        let to_key = Self::path_to_key(to);

        let storage_read = self.storage.read().unwrap();

        if storage_read.contains_key(&to_key) {
            return Err(Error::AlreadyExists(format!(
                "Destination already exists: {}",
                to
            )));
        }

        let entry = storage_read
            .get(&from_key)
            .ok_or_else(|| Error::NotFound(format!("Source not found: {}", from)))?;

        let new_entry = match entry {
            Entry::File { data, .. } => {
                let metadata = Metadata {
                    id: Uuid::new_v4().to_string(),
                    name: to.name().unwrap_or("/").to_string(),
                    size: Some(data.len() as u64),
                    is_directory: false,
                    modified: Utc::now(),
                    etag: Some(Uuid::new_v4().to_string()),
                    provider_data: None,
                };
                Entry::File {
                    data: data.clone(),
                    metadata,
                }
            }
            Entry::Directory { .. } => {
                let metadata = Metadata {
                    id: Uuid::new_v4().to_string(),
                    name: to.name().unwrap_or("/").to_string(),
                    size: None,
                    is_directory: true,
                    modified: Utc::now(),
                    etag: Some(Uuid::new_v4().to_string()),
                    provider_data: None,
                };
                Entry::Directory { metadata }
            }
        };

        let result_metadata = match &new_entry {
            Entry::File { metadata, .. } => metadata.clone(),
            Entry::Directory { metadata } => metadata.clone(),
        };

        drop(storage_read);
        self.storage.write().unwrap().insert(to_key, new_entry);

        Ok(result_metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_upload_download() {
        let provider = MemoryProvider::new();
        let path = VaultPath::parse("/test.txt").unwrap();
        let data = b"Hello, World!".to_vec();

        provider.upload(&path, data.clone()).await.unwrap();
        let downloaded = provider.download(&path).await.unwrap();

        assert_eq!(downloaded, data);
    }

    #[tokio::test]
    async fn test_exists() {
        let provider = MemoryProvider::new();
        let path = VaultPath::parse("/test.txt").unwrap();

        assert!(!provider.exists(&path).await.unwrap());

        provider.upload(&path, vec![1, 2, 3]).await.unwrap();

        assert!(provider.exists(&path).await.unwrap());
    }

    #[tokio::test]
    async fn test_delete() {
        let provider = MemoryProvider::new();
        let path = VaultPath::parse("/test.txt").unwrap();

        provider.upload(&path, vec![1, 2, 3]).await.unwrap();
        assert!(provider.exists(&path).await.unwrap());

        provider.delete(&path).await.unwrap();
        assert!(!provider.exists(&path).await.unwrap());
    }

    #[tokio::test]
    async fn test_create_dir() {
        let provider = MemoryProvider::new();
        let path = VaultPath::parse("/mydir").unwrap();

        let metadata = provider.create_dir(&path).await.unwrap();
        assert!(metadata.is_directory);
        assert_eq!(metadata.name, "mydir");
    }

    #[tokio::test]
    async fn test_list() {
        let provider = MemoryProvider::new();

        provider.create_dir(&VaultPath::parse("/dir").unwrap()).await.unwrap();
        provider.upload(&VaultPath::parse("/dir/file1.txt").unwrap(), vec![1]).await.unwrap();
        provider.upload(&VaultPath::parse("/dir/file2.txt").unwrap(), vec![2]).await.unwrap();

        let contents = provider.list(&VaultPath::parse("/dir").unwrap()).await.unwrap();
        assert_eq!(contents.len(), 2);
    }

    #[tokio::test]
    async fn test_rename() {
        let provider = MemoryProvider::new();
        let from = VaultPath::parse("/old.txt").unwrap();
        let to = VaultPath::parse("/new.txt").unwrap();

        provider.upload(&from, vec![1, 2, 3]).await.unwrap();
        provider.rename(&from, &to).await.unwrap();

        assert!(!provider.exists(&from).await.unwrap());
        assert!(provider.exists(&to).await.unwrap());
    }

    #[tokio::test]
    async fn test_copy() {
        let provider = MemoryProvider::new();
        let from = VaultPath::parse("/original.txt").unwrap();
        let to = VaultPath::parse("/copy.txt").unwrap();
        let data = vec![1, 2, 3];

        provider.upload(&from, data.clone()).await.unwrap();
        provider.copy(&from, &to).await.unwrap();

        assert!(provider.exists(&from).await.unwrap());
        assert!(provider.exists(&to).await.unwrap());
        assert_eq!(provider.download(&to).await.unwrap(), data);
    }
}
