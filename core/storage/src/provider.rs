//! Storage provider trait definition.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

use axiomvault_common::{Result, VaultPath};

/// Metadata for a stored object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    /// Unique identifier for the object (provider-specific).
    pub id: String,
    /// Name of the object.
    pub name: String,
    /// Size in bytes (None for directories).
    pub size: Option<u64>,
    /// Whether this is a directory.
    pub is_directory: bool,
    /// Last modification time.
    pub modified: DateTime<Utc>,
    /// ETag or revision ID for conflict detection.
    pub etag: Option<String>,
    /// Provider-specific metadata.
    pub provider_data: Option<serde_json::Value>,
}

/// Conflict resolution strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictResolution {
    /// Keep both versions (rename local).
    KeepBoth,
    /// Prefer local version.
    PreferLocal,
    /// Prefer remote version.
    PreferRemote,
}

/// Byte stream type for upload/download operations.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Vec<u8>>> + Send>>;

/// Storage provider trait for different backends.
///
/// All operations are async and use streams for large data transfers.
/// Implementations must handle their own authentication and rate limiting.
#[async_trait]
pub trait StorageProvider: Send + Sync {
    /// Get the provider name (e.g., "gdrive", "local", "icloud").
    fn name(&self) -> &str;

    /// Upload data to the storage.
    ///
    /// # Preconditions
    /// - Parent directory must exist
    /// - `data` is the complete content to upload
    ///
    /// # Postconditions
    /// - File is created or updated at the specified path
    /// - Returns metadata of the uploaded file
    ///
    /// # Errors
    /// - Parent directory not found
    /// - Network/I/O errors
    /// - Authentication errors
    async fn upload(&self, path: &VaultPath, data: Vec<u8>) -> Result<Metadata>;

    /// Upload data as a stream.
    ///
    /// For large files, this allows streaming without loading entire file into memory.
    async fn upload_stream(&self, path: &VaultPath, stream: ByteStream) -> Result<Metadata>;

    /// Download data from storage.
    ///
    /// # Preconditions
    /// - File must exist at path
    ///
    /// # Postconditions
    /// - Returns complete file content
    ///
    /// # Errors
    /// - File not found
    /// - Network/I/O errors
    async fn download(&self, path: &VaultPath) -> Result<Vec<u8>>;

    /// Download data as a stream.
    ///
    /// For large files, this allows streaming without loading entire file into memory.
    async fn download_stream(&self, path: &VaultPath) -> Result<ByteStream>;

    /// Check if a path exists.
    async fn exists(&self, path: &VaultPath) -> Result<bool>;

    /// Delete a file.
    ///
    /// # Errors
    /// - File not found
    /// - Not permitted (e.g., directory)
    async fn delete(&self, path: &VaultPath) -> Result<()>;

    /// List contents of a directory.
    ///
    /// # Preconditions
    /// - Path must be a directory
    ///
    /// # Returns
    /// Vector of metadata for each item in the directory.
    async fn list(&self, path: &VaultPath) -> Result<Vec<Metadata>>;

    /// Get metadata for a path.
    ///
    /// # Errors
    /// - Path not found
    async fn metadata(&self, path: &VaultPath) -> Result<Metadata>;

    /// Create a directory.
    ///
    /// # Postconditions
    /// - Directory is created (including parents if needed)
    ///
    /// # Errors
    /// - Already exists as file
    /// - Permission denied
    async fn create_dir(&self, path: &VaultPath) -> Result<Metadata>;

    /// Delete a directory.
    ///
    /// # Preconditions
    /// - Directory must be empty (or recursive=true)
    async fn delete_dir(&self, path: &VaultPath) -> Result<()>;

    /// Move/rename a path.
    async fn rename(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata>;

    /// Copy a path.
    async fn copy(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata>;
}

/// Extension trait for conflict detection.
#[async_trait]
pub trait ConflictAware: StorageProvider {
    /// Check if there's a conflict with the remote version.
    ///
    /// Uses etag/revision comparison to detect conflicts.
    async fn has_conflict(&self, path: &VaultPath, local_etag: &str) -> Result<bool>;

    /// Resolve a conflict using the specified strategy.
    async fn resolve_conflict(
        &self,
        path: &VaultPath,
        local_data: Vec<u8>,
        strategy: ConflictResolution,
    ) -> Result<Metadata>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_serialization() {
        let metadata = Metadata {
            id: "test-id".to_string(),
            name: "test-file.txt".to_string(),
            size: Some(1024),
            is_directory: false,
            modified: Utc::now(),
            etag: Some("abc123".to_string()),
            provider_data: None,
        };

        let json = serde_json::to_string(&metadata).unwrap();
        let deserialized: Metadata = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, metadata.id);
        assert_eq!(deserialized.name, metadata.name);
        assert_eq!(deserialized.size, metadata.size);
    }
}
