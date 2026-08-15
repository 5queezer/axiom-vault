//! Vault file operations with encryption/decryption.

use std::io::Write;
use std::path::Path;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use futures::StreamExt;
use tracing::{debug, info};
use zeroize::Zeroizing;

use crate::config::DATA_DIRNAME;
use crate::session::VaultSession;
use axiomvault_common::{Error, Result, VaultPath};
use axiomvault_crypto::{decrypt, encrypt, DecryptingStream, EncryptingStream};
use axiomvault_storage::ByteStream;

const STREAM_CONTENT_VERSION: u8 = 2;

fn persist_plaintext_noclobber(destination: &Path, plaintext: &[u8]) -> Result<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| Error::InvalidInput("Destination has no parent".to_string()))?;
    #[cfg(not(unix))]
    {
        let _ = (parent, plaintext);
        return Err(Error::NotPermitted(
            "secure plaintext-file ACL creation is not implemented on this platform".to_string(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut output = tempfile::NamedTempFile::new_in(parent)?;
        output
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
        output.write_all(plaintext)?;
        output.as_file().sync_all()?;
        output
            .persist_noclobber(destination)
            .map_err(|error| Error::Io(error.error))?;
        Ok(())
    }
}

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

        debug!("Creating encrypted file");

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

        info!(size = content.len(), "File created");
        Ok(())
    }

    /// Import a local file through bounded-memory encryption and provider streaming.
    pub async fn create_file_from_path(
        &self,
        path: &VaultPath,
        source: impl AsRef<Path>,
    ) -> Result<()> {
        let name = path
            .name()
            .ok_or_else(|| Error::InvalidInput("Invalid file path".to_string()))?;
        if self.session.tree().read().await.get_node(path).is_ok() {
            return Err(Error::AlreadyExists(format!("Path already exists: {path}")));
        }
        let source = source.as_ref().to_path_buf();
        let size = tokio::fs::metadata(&source).await?.len();
        let encrypted_name = self.encrypt_name(name)?;
        let file_key = self
            .session
            .master_key()?
            .derive_file_key(encrypted_name.as_bytes());
        let key = Zeroizing::new(file_key.as_bytes().to_vec());

        let encrypted_file =
            tokio::task::spawn_blocking(move || -> Result<tempfile::NamedTempFile> {
                let mut input = std::fs::File::open(source)?;
                let mut output = tempfile::NamedTempFile::new()?;
                EncryptingStream::new(&key)?.encrypt_stream(&mut input, output.as_file_mut())?;
                output.as_file().sync_all()?;
                Ok(output)
            })
            .await
            .map_err(|error| Error::Storage(format!("encryption task failed: {}", error)))??;

        let encrypted_size = tokio::fs::metadata(encrypted_file.path()).await?.len();
        let file = tokio::fs::File::open(encrypted_file.path()).await?;
        let encrypted_stream: ByteStream = Box::pin(
            tokio_util::io::ReaderStream::with_capacity(file, 64 * 1024)
                .map(|item| item.map(|bytes| bytes.to_vec()).map_err(Error::from)),
        );
        let storage_path = VaultPath::parse(DATA_DIRNAME)?.join(&encrypted_name)?;
        self.session
            .provider()
            .upload_sized_stream(&storage_path, encrypted_stream, encrypted_size)
            .await?;

        {
            let mut tree = self.session.tree().write().await;
            tree.create_file(path, &encrypted_name, size)?;
            tree.get_node_mut(path)?.metadata.content_version = STREAM_CONTENT_VERSION;
        }
        self.session.save_tree().await?;
        info!(size, "File imported");
        Ok(())
    }

    /// Export a vault file without materializing ciphertext or plaintext in memory.
    /// Plaintext is written to a temporary sibling and only published after every
    /// frame, including the authenticated end frame, has verified successfully.
    pub async fn export_file_to_path(
        &self,
        path: &VaultPath,
        destination: impl AsRef<Path>,
    ) -> Result<()> {
        let (encrypted_name, content_version) = {
            let tree = self.session.tree().read().await;
            let node = tree.get_node(path)?;
            if !node.is_file() {
                return Err(Error::InvalidInput("Not a file".to_string()));
            }
            (
                node.metadata.encrypted_name.clone(),
                node.metadata.content_version,
            )
        };
        if content_version == 0 {
            // Existing vaults predate stream framing. Preserve deterministic
            // compatibility instead of guessing from random nonce bytes. This
            // one-time legacy path uses the old whole-object API; rewriting the
            // file through import/update migrates it to version 2.
            let content = self.read_file(path).await?;
            let destination = destination.as_ref().to_path_buf();
            return tokio::task::spawn_blocking(move || {
                persist_plaintext_noclobber(&destination, &content)
            })
            .await
            .map_err(|error| Error::Storage(format!("plaintext write task failed: {error}")))?;
        }
        if content_version != STREAM_CONTENT_VERSION {
            return Err(Error::Crypto(format!(
                "Unsupported file content version: {}",
                content_version
            )));
        }
        let storage_path = VaultPath::parse(DATA_DIRNAME)?.join(&encrypted_name)?;
        let mut stream = self
            .session
            .provider()
            .download_stream(&storage_path)
            .await?;
        let encrypted_file = tempfile::NamedTempFile::new()?;
        let mut encrypted_output = tokio::fs::File::create(encrypted_file.path()).await?;
        use tokio::io::AsyncWriteExt;
        while let Some(chunk) = stream.next().await {
            encrypted_output.write_all(&chunk?).await?;
        }
        encrypted_output.sync_all().await?;
        drop(encrypted_output);

        let destination = destination.as_ref().to_path_buf();
        let parent = destination
            .parent()
            .ok_or_else(|| Error::InvalidInput("Destination has no parent".to_string()))?
            .to_path_buf();
        let file_key = self
            .session
            .master_key()?
            .derive_file_key(encrypted_name.as_bytes());
        let key = Zeroizing::new(file_key.as_bytes().to_vec());
        tokio::task::spawn_blocking(move || -> Result<()> {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut input = std::fs::File::open(encrypted_file.path())?;
                let mut output = tempfile::NamedTempFile::new_in(parent)?;
                output
                    .as_file()
                    .set_permissions(std::fs::Permissions::from_mode(0o600))?;
                DecryptingStream::new(&key)?.decrypt_stream(&mut input, output.as_file_mut())?;
                output.as_file().sync_all()?;
                output
                    .persist_noclobber(destination)
                    .map_err(|error| Error::Io(error.error))?;
                Ok(())
            }
            #[cfg(not(unix))]
            {
                let _ = (encrypted_file, parent, destination, key);
                Err(Error::NotPermitted(
                    "secure plaintext-file ACL creation is not implemented on this platform"
                        .to_string(),
                ))
            }
        })
        .await
        .map_err(|error| Error::Storage(format!("decryption task failed: {}", error)))??;
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
        debug!("Reading encrypted file");

        let (encrypted_name, content_version) = {
            let tree = self.session.tree().read().await;
            let node = tree.get_node(path)?;
            if !node.is_file() {
                return Err(Error::InvalidInput("Not a file".to_string()));
            }
            (
                node.metadata.encrypted_name.clone(),
                node.metadata.content_version,
            )
        };

        let storage_path = VaultPath::parse(DATA_DIRNAME)?.join(&encrypted_name)?;
        let encrypted_content = self.session.provider().download(&storage_path).await?;

        let master_key = self.session.master_key()?;
        let file_key = master_key.derive_file_key(encrypted_name.as_bytes());
        let content = match content_version {
            0 => decrypt(file_key.as_bytes(), &encrypted_content)?,
            STREAM_CONTENT_VERSION => {
                let mut plaintext = Vec::new();
                DecryptingStream::new(file_key.as_bytes())?
                    .decrypt_stream(encrypted_content.as_slice(), &mut plaintext)?;
                plaintext
            }
            other => {
                return Err(Error::Crypto(format!(
                    "Unsupported file content version: {other}"
                )))
            }
        };

        debug!(size = content.len(), "File read");
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
        debug!("Updating encrypted file");

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
            node.metadata.content_version = 0;
        }

        let storage_path = VaultPath::parse(DATA_DIRNAME)?.join(&encrypted_name)?;
        self.session
            .provider()
            .upload(&storage_path, encrypted_content)
            .await?;

        self.session.save_tree().await?;

        info!(size = content.len(), "File updated");
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
        debug!("Deleting file");

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

        info!("File deleted");
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

        debug!("Creating directory");

        let encrypted_name = self.encrypt_name(name)?;

        {
            let mut tree = self.session.tree().write().await;
            tree.create_directory(path, &encrypted_name)?;
        }

        self.session.save_tree().await?;

        info!("Directory created");
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
        debug!("Deleting directory");

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

        info!("Directory deleted");
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
        let creation =
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
        VaultSession::unlock(creation.config, password, provider, VaultTree::new()).unwrap()
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
    async fn legacy_one_shot_file_exports_via_versioned_fallback() {
        let session = create_test_session().await;
        let ops = VaultOperations::new(&session).unwrap();
        let temp = tempfile::TempDir::new().unwrap();
        let destination = temp.path().join("legacy.txt");
        let path = VaultPath::parse("/legacy.txt").unwrap();
        ops.create_file(&path, b"legacy content").await.unwrap();

        ops.export_file_to_path(&path, &destination).await.unwrap();

        assert_eq!(std::fs::read(destination).unwrap(), b"legacy content");
    }

    #[tokio::test]
    async fn test_large_file_path_roundtrip_uses_stream_api() {
        let session = create_test_session().await;
        let ops = VaultOperations::new(&session).unwrap();
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("destination.bin");
        let data = vec![0x5a; 8 * 1024 * 1024];
        std::fs::write(&source, &data).unwrap();
        let path = VaultPath::parse("/large.bin").unwrap();

        ops.create_file_from_path(&path, &source).await.unwrap();
        ops.export_file_to_path(&path, &destination).await.unwrap();

        assert_eq!(std::fs::read(destination).unwrap(), data);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn valid_stream_export_does_not_clobber_existing_destination() {
        let session = create_test_session().await;
        let ops = VaultOperations::new(&session).unwrap();
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("destination.bin");
        std::fs::write(&source, vec![0x5a; 256 * 1024]).unwrap();
        std::fs::write(&destination, b"existing safe content").unwrap();
        let path = VaultPath::parse("/noclobber.bin").unwrap();
        ops.create_file_from_path(&path, &source).await.unwrap();

        assert!(ops.export_file_to_path(&path, &destination).await.is_err());
        assert_eq!(
            std::fs::read(destination).unwrap(),
            b"existing safe content"
        );
    }

    #[tokio::test]
    async fn tampered_chunk_does_not_publish_partial_plaintext() {
        let session = create_test_session().await;
        let ops = VaultOperations::new(&session).unwrap();
        let temp = tempfile::TempDir::new().unwrap();
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("destination.bin");
        std::fs::write(&source, vec![0x5a; 256 * 1024]).unwrap();
        std::fs::write(&destination, b"existing safe content").unwrap();
        let path = VaultPath::parse("/tampered.bin").unwrap();
        ops.create_file_from_path(&path, &source).await.unwrap();

        let data_dir = VaultPath::parse("/d").unwrap();
        let stored = session.provider().list(&data_dir).await.unwrap();
        let stored_path = data_dir.join(&stored[0].name).unwrap();
        let mut ciphertext = session.provider().download(&stored_path).await.unwrap();
        let middle = ciphertext.len() / 2;
        ciphertext[middle] ^= 0x80;
        session
            .provider()
            .upload(&stored_path, ciphertext)
            .await
            .unwrap();

        assert!(ops.export_file_to_path(&path, &destination).await.is_err());
        assert_eq!(
            std::fs::read(destination).unwrap(),
            b"existing safe content"
        );
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
