//! Vault file operations with encryption/decryption.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use tracing::{debug, info};

use crate::config::DATA_DIRNAME;
use crate::session::VaultSession;
use axiomvault_common::{Error, Result, VaultPath};
use axiomvault_crypto::{decrypt, encrypt};

/// Vault operations handler.
///
/// Provides encrypted file operations using an active session.
pub struct VaultOperations<'a> {
    session: &'a VaultSession,
}

impl<'a> VaultOperations<'a> {
    /// Create new operations handler for a session.
    pub fn new(session: &'a VaultSession) -> Result<Self> {
        if !session.is_active() {
            return Err(Error::NotPermitted("Session is not active".to_string()));
        }
        Ok(Self { session })
    }

    /// Encrypt a filename.
    fn encrypt_name(&self, name: &str) -> Result<String> {
        let master_key = self.session.master_key()?;
        let dir_key = master_key.derive_directory_key(b"names");
        let encrypted = encrypt(dir_key.as_bytes(), name.as_bytes())?;
        Ok(URL_SAFE_NO_PAD.encode(encrypted))
    }

    /// Create a new file with encrypted content.
    ///
    /// # Preconditions
    /// - Parent directory must exist
    /// - File must not exist
    /// - Session must be active
    ///
    /// # Postconditions
    /// - File is created in storage with encrypted content
    /// - Tree is updated with new file entry
    ///
    /// # Errors
    /// - Parent not found
    /// - File already exists
    /// - Encryption failure
    /// - Storage failure
    pub async fn create_file(&self, path: &VaultPath, content: &[u8]) -> Result<()> {
        let name = path
            .name()
            .ok_or_else(|| Error::InvalidInput("Invalid file path".to_string()))?;

        debug!(path = %path, "Creating encrypted file");

        let encrypted_name = self.encrypt_name(name)?;

        let master_key = self.session.master_key()?;
        let file_key = master_key.derive_file_key(encrypted_name.as_bytes());
        let encrypted_content = encrypt(file_key.as_bytes(), content)?;

        {
            let mut tree = self.session.tree().write().await;
            tree.create_file(path, &encrypted_name, content.len() as u64)?;
        }

        let storage_path = VaultPath::parse(DATA_DIRNAME)?.join(&encrypted_name)?;
        self.session
            .provider()
            .upload(&storage_path, encrypted_content)
            .await?;

        self.session.save_tree().await?;

        info!(path = %path, size = content.len(), "File created");
        Ok(())
    }

    /// Read and decrypt file content.
    ///
    /// # Preconditions
    /// - File must exist
    /// - Session must be active
    ///
    /// # Postconditions
    /// - Returns decrypted file content
    ///
    /// # Errors
    /// - File not found
    /// - Decryption failure
    /// - Storage failure
    pub async fn read_file(&self, path: &VaultPath) -> Result<Vec<u8>> {
        debug!(path = %path, "Reading encrypted file");

        let encrypted_name = {
            let tree = self.session.tree().read().await;
            let node = tree.get_node(path)?;
            if !node.is_file() {
                return Err(Error::InvalidInput("Not a file".to_string()));
            }
            node.metadata.encrypted_name.clone()
        };

        let storage_path = VaultPath::parse(DATA_DIRNAME)?.join(&encrypted_name)?;
        let encrypted_content = self.session.provider().download(&storage_path).await?;

        let master_key = self.session.master_key()?;
        let file_key = master_key.derive_file_key(encrypted_name.as_bytes());
        let content = decrypt(file_key.as_bytes(), &encrypted_content)?;

        debug!(path = %path, size = content.len(), "File read");
        Ok(content)
    }

    /// Update file with new encrypted content.
    ///
    /// # Preconditions
    /// - File must exist
    /// - Session must be active
    ///
    /// # Postconditions
    /// - File content is updated with new encrypted data
    /// - Tree metadata is updated
    ///
    /// # Errors
    /// - File not found
    /// - Encryption failure
    /// - Storage failure
    pub async fn update_file(&self, path: &VaultPath, content: &[u8]) -> Result<()> {
        debug!(path = %path, "Updating encrypted file");

        let encrypted_name = {
            let tree = self.session.tree().read().await;
            let node = tree.get_node(path)?;
            if !node.is_file() {
                return Err(Error::InvalidInput("Not a file".to_string()));
            }
            node.metadata.encrypted_name.clone()
        };

        let master_key = self.session.master_key()?;
        let file_key = master_key.derive_file_key(encrypted_name.as_bytes());
        let encrypted_content = encrypt(file_key.as_bytes(), content)?;

        {
            let mut tree = self.session.tree().write().await;
            let node = tree.get_node_mut(path)?;
            node.metadata.size = Some(content.len() as u64);
            node.metadata.modified_at = chrono::Utc::now();
        }

        let storage_path = VaultPath::parse(DATA_DIRNAME)?.join(&encrypted_name)?;
        self.session
            .provider()
            .upload(&storage_path, encrypted_content)
            .await?;

        self.session.save_tree().await?;

        info!(path = %path, size = content.len(), "File updated");
        Ok(())
    }

    /// Delete a file.
    ///
    /// # Preconditions
    /// - File must exist
    ///
    /// # Postconditions
    /// - File is removed from storage
    /// - Tree entry is removed
    ///
    /// # Errors
    /// - File not found
    /// - Storage failure
    pub async fn delete_file(&self, path: &VaultPath) -> Result<()> {
        debug!(path = %path, "Deleting file");

        let encrypted_name = {
            let mut tree = self.session.tree().write().await;
            let node = tree.get_node(path)?;
            if !node.is_file() {
                return Err(Error::InvalidInput("Not a file".to_string()));
            }
            let name = node.metadata.encrypted_name.clone();
            tree.remove(path)?;
            name
        };

        let storage_path = VaultPath::parse(DATA_DIRNAME)?.join(&encrypted_name)?;
        self.session.provider().delete(&storage_path).await?;

        self.session.save_tree().await?;

        info!(path = %path, "File deleted");
        Ok(())
    }

    /// Create a directory.
    ///
    /// # Preconditions
    /// - Parent must exist
    /// - Directory must not exist
    ///
    /// # Postconditions
    /// - Directory is created in tree
    /// - Directory metadata is stored
    ///
    /// # Errors
    /// - Parent not found
    /// - Already exists
    pub async fn create_directory(&self, path: &VaultPath) -> Result<()> {
        let name = path
            .name()
            .ok_or_else(|| Error::InvalidInput("Invalid directory path".to_string()))?;

        debug!(path = %path, "Creating directory");

        let encrypted_name = self.encrypt_name(name)?;

        {
            let mut tree = self.session.tree().write().await;
            tree.create_directory(path, &encrypted_name)?;
        }

        self.session.save_tree().await?;

        info!(path = %path, "Directory created");
        Ok(())
    }

    /// List directory contents.
    ///
    /// # Preconditions
    /// - Path must be a directory
    ///
    /// # Returns
    /// List of (name, is_directory, size) tuples.
    pub async fn list_directory(
        &self,
        path: &VaultPath,
    ) -> Result<Vec<(String, bool, Option<u64>)>> {
        let tree = self.session.tree().read().await;
        let contents = tree.list(path)?;

        Ok(contents
            .iter()
            .map(|node| {
                (
                    node.metadata.name.clone(),
                    node.is_directory(),
                    node.metadata.size,
                )
            })
            .collect())
    }

    /// Delete an empty directory.
    ///
    /// # Preconditions
    /// - Path must be a directory
    /// - Directory must be empty
    ///
    /// # Errors
    /// - Not a directory
    /// - Directory not empty
    pub async fn delete_directory(&self, path: &VaultPath) -> Result<()> {
        debug!(path = %path, "Deleting directory");

        {
            let mut tree = self.session.tree().write().await;
            let node = tree.get_node(path)?;

            if !node.is_directory() {
                return Err(Error::InvalidInput("Not a directory".to_string()));
            }

            if !node.children.is_empty() {
                return Err(Error::InvalidInput("Directory not empty".to_string()));
            }

            tree.remove(path)?;
        }

        self.session.save_tree().await?;

        info!(path = %path, "Directory deleted");
        Ok(())
    }

    /// Check if path exists.
    pub async fn exists(&self, path: &VaultPath) -> bool {
        let tree = self.session.tree().read().await;
        tree.exists(path)
    }

    /// Get metadata for a path.
    pub async fn metadata(&self, path: &VaultPath) -> Result<(String, bool, Option<u64>)> {
        let tree = self.session.tree().read().await;
        let node = tree.get_node(path)?;
        Ok((
            node.metadata.name.clone(),
            node.is_directory(),
            node.metadata.size,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::VaultConfig;
    use axiomvault_common::VaultId;
    use axiomvault_crypto::KdfParams;
    use axiomvault_storage::{MemoryProvider, StorageProvider};
    use std::sync::Arc;

    async fn create_test_session() -> VaultSession {
        let id = VaultId::new("test").unwrap();
        let password = b"test-password";
        let params = KdfParams::moderate();
        let config =
            VaultConfig::new(id, password, "memory", serde_json::Value::Null, params).unwrap();

        let provider = Arc::new(MemoryProvider::new());

        provider
            .create_dir(&VaultPath::parse("/d").unwrap())
            .await
            .unwrap();

        provider
            .create_dir(&VaultPath::parse("/m").unwrap())
            .await
            .unwrap();

        use crate::tree::VaultTree;
        VaultSession::unlock(config, password, provider, VaultTree::new()).unwrap()
    }

    #[tokio::test]
    async fn test_create_and_read_file() {
        let session = create_test_session().await;
        let ops = VaultOperations::new(&session).unwrap();

        let path = VaultPath::parse("/test.txt").unwrap();
        let content = b"Hello, encrypted world!";

        ops.create_file(&path, content).await.unwrap();
        let read_content = ops.read_file(&path).await.unwrap();

        assert_eq!(read_content, content);
    }

    #[tokio::test]
    async fn test_update_file() {
        let session = create_test_session().await;
        let ops = VaultOperations::new(&session).unwrap();

        let path = VaultPath::parse("/test.txt").unwrap();
        ops.create_file(&path, b"initial").await.unwrap();
        ops.update_file(&path, b"updated").await.unwrap();

        let content = ops.read_file(&path).await.unwrap();
        assert_eq!(content, b"updated");
    }

    #[tokio::test]
    async fn test_delete_file() {
        let session = create_test_session().await;
        let ops = VaultOperations::new(&session).unwrap();

        let path = VaultPath::parse("/test.txt").unwrap();
        ops.create_file(&path, b"content").await.unwrap();
        assert!(ops.exists(&path).await);

        ops.delete_file(&path).await.unwrap();
        assert!(!ops.exists(&path).await);
    }

    #[tokio::test]
    async fn test_create_directory() {
        let session = create_test_session().await;
        let ops = VaultOperations::new(&session).unwrap();

        let path = VaultPath::parse("/mydir").unwrap();
        ops.create_directory(&path).await.unwrap();

        let (name, is_dir, _) = ops.metadata(&path).await.unwrap();
        assert_eq!(name, "mydir");
        assert!(is_dir);
    }

    #[tokio::test]
    async fn test_list_directory() {
        let session = create_test_session().await;
        let ops = VaultOperations::new(&session).unwrap();

        ops.create_directory(&VaultPath::parse("/dir").unwrap())
            .await
            .unwrap();
        ops.create_file(&VaultPath::parse("/dir/a.txt").unwrap(), b"a")
            .await
            .unwrap();
        ops.create_file(&VaultPath::parse("/dir/b.txt").unwrap(), b"b")
            .await
            .unwrap();

        let contents = ops
            .list_directory(&VaultPath::parse("/dir").unwrap())
            .await
            .unwrap();
        assert_eq!(contents.len(), 2);
    }
}
