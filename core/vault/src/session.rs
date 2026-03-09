//! Vault session management.
//!
//! Sessions hold decrypted keys in memory and provide access to vault operations.
//! Keys are automatically zeroized when the session is dropped.

use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::config::{VaultConfig, META_DIRNAME, TREE_FILENAME};
use crate::tree::VaultTree;
use axiomvault_common::{Error, Result, VaultId, VaultPath};
use axiomvault_crypto::recovery::RecoveryKey;
use axiomvault_crypto::{decrypt, derive_key, encrypt, MasterKey};
use axiomvault_storage::StorageProvider;

/// Context tag for tree index key derivation. Changing this invalidates all existing vaults.
const TREE_KEY_CONTEXT: &[u8] = b"vault_tree_index_v1";

/// Session handle for tracking active sessions.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionHandle(String);

impl SessionHandle {
    /// Generate a new unique session handle.
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// Get the handle string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SessionHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// State of the vault session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Session is active and keys are available.
    Active,
    /// Session is locked, keys have been cleared.
    Locked,
}

/// Active vault session.
///
/// Holds the master key and provides access to vault operations.
/// The master key is zeroized when the session is dropped or locked.
pub struct VaultSession {
    /// Unique session identifier.
    handle: SessionHandle,
    /// Vault configuration.
    config: VaultConfig,
    /// Master key (zeroized on drop).
    master_key: Option<MasterKey>,
    /// Storage provider.
    provider: Arc<dyn StorageProvider>,
    /// Cached vault tree.
    tree: Arc<RwLock<VaultTree>>,
    /// Session state.
    state: SessionState,
}

impl VaultSession {
    /// Create a new vault session from an already-derived master key.
    ///
    /// This is the preferred constructor for `open_vault` paths where the key has
    /// already been derived (verified against `key_verification`) so we avoid
    /// running the expensive Argon2id KDF a second time.
    ///
    /// # Errors
    /// - Incompatible vault version
    pub fn from_master_key(
        config: VaultConfig,
        master_key: MasterKey,
        provider: Arc<dyn StorageProvider>,
        tree: VaultTree,
    ) -> Result<Self> {
        if !config.version.is_compatible() {
            return Err(Error::Vault(format!(
                "Incompatible vault version: {:?}",
                config.version
            )));
        }

        Ok(Self {
            handle: SessionHandle::new(),
            config,
            master_key: Some(master_key),
            provider,
            tree: Arc::new(RwLock::new(tree)),
            state: SessionState::Active,
        })
    }

    /// Create a new vault session by unlocking with password.
    ///
    /// Derives the master key via Argon2id. Prefer `from_master_key` when the
    /// key has already been derived (e.g. in `open_vault`), to avoid a double KDF.
    pub fn unlock(
        config: VaultConfig,
        password: &[u8],
        provider: Arc<dyn StorageProvider>,
        tree: VaultTree,
    ) -> Result<Self> {
        let master_key = config
            .verify_password(password)?
            .ok_or_else(|| Error::NotPermitted("Invalid password".to_string()))?;

        Self::from_master_key(config, master_key, provider, tree)
    }

    /// Load and decrypt the vault tree index from storage.
    pub async fn load_and_decrypt_tree(
        provider: &Arc<dyn StorageProvider>,
        master_key: &MasterKey,
    ) -> Result<VaultTree> {
        let tree_path = VaultPath::parse(META_DIRNAME)?.join(TREE_FILENAME)?;

        if !provider.exists(&tree_path).await? {
            return Ok(VaultTree::new());
        }

        let encrypted_bytes = provider.download(&tree_path).await?;

        let tree_key = master_key.derive_file_key(TREE_KEY_CONTEXT);
        let tree_bytes = decrypt(tree_key.as_bytes(), &encrypted_bytes).map_err(|e| {
            Error::Crypto(format!(
                "Failed to decrypt tree index (wrong password or corrupted vault): {}",
                e
            ))
        })?;

        let tree_json = String::from_utf8(tree_bytes)
            .map_err(|e| Error::Serialization(format!("Invalid UTF-8 in tree data: {}", e)))?;

        VaultTree::from_json(&tree_json)
    }

    /// Get the session handle.
    pub fn handle(&self) -> &SessionHandle {
        &self.handle
    }

    /// Get the vault ID.
    pub fn vault_id(&self) -> &VaultId {
        &self.config.id
    }

    /// Get the vault configuration.
    pub fn config(&self) -> &VaultConfig {
        &self.config
    }

    /// Get mutable reference to the vault configuration.
    pub fn config_mut(&mut self) -> &mut VaultConfig {
        &mut self.config
    }

    /// Get the storage provider.
    pub fn provider(&self) -> Arc<dyn StorageProvider> {
        self.provider.clone()
    }

    /// Get reference to the vault tree.
    pub fn tree(&self) -> &Arc<RwLock<VaultTree>> {
        &self.tree
    }

    /// Get the master key, if session is active.
    pub fn master_key(&self) -> Result<&MasterKey> {
        match self.state {
            SessionState::Active => self
                .master_key
                .as_ref()
                .ok_or_else(|| Error::Vault("Master key not available".to_string())),
            SessionState::Locked => Err(Error::NotPermitted("Session is locked".to_string())),
        }
    }

    /// Get the current session state.
    pub fn state(&self) -> SessionState {
        self.state
    }

    /// Check if session is active.
    pub fn is_active(&self) -> bool {
        self.state == SessionState::Active
    }

    /// Lock the session, clearing all keys from memory.
    pub fn lock(&mut self) {
        if let Some(key) = self.master_key.take() {
            drop(key);
        }
        self.state = SessionState::Locked;
    }

    /// Change the vault password.
    ///
    /// Re-wraps the master key with a new password-derived KEK.
    /// Recovery key data remains unchanged.
    pub fn change_password(&mut self, old_password: &[u8], new_password: &[u8]) -> Result<()> {
        use axiomvault_crypto::recovery::wrap_key;

        if self.state != SessionState::Active {
            return Err(Error::NotPermitted("Session is locked".to_string()));
        }

        // verify_password returns the master key — use it directly to avoid
        // a second call to self.master_key().
        let master_key = self
            .config
            .verify_password(old_password)?
            .ok_or_else(|| Error::NotPermitted("Invalid old password".to_string()))?;

        // Generate new salt and derive new password KEK.
        let new_salt = axiomvault_crypto::Salt::generate();
        let new_kek = derive_key(new_password, &new_salt, &self.config.kdf_params)?;

        // Re-wrap the master key with the new KEK.
        let new_wrapped = wrap_key(&master_key, new_kek.as_bytes())?;

        // Re-create password verification.
        let verification_plaintext = b"AXIOMVAULT_KEY_VERIFICATION_V1";
        let new_verification = encrypt(new_kek.as_bytes(), verification_plaintext)?;

        self.config.salt = new_salt;
        self.config.key_verification = new_verification;
        self.config.wrapped_master_key = Some(new_wrapped);
        self.config.modified_at = chrono::Utc::now();

        // Master key itself does not change.

        Ok(())
    }

    /// Reset password using a recovery key.
    ///
    /// Unwraps the master key using the recovery key and re-wraps it
    /// with a new password-derived KEK.
    pub fn reset_password_with_recovery(
        &mut self,
        recovery_key: &RecoveryKey,
        new_password: &[u8],
    ) -> Result<()> {
        // Get the master key from recovery before resetting password.
        // This avoids a second Argon2id round after reset_password.
        let master_key = self
            .config
            .verify_recovery_key(recovery_key)?
            .ok_or_else(|| Error::NotPermitted("Invalid recovery key".to_string()))?;

        self.config.reset_password(recovery_key, new_password)?;
        self.master_key = Some(master_key);
        self.state = SessionState::Active;

        Ok(())
    }

    /// Save the current tree state to storage (encrypted).
    pub async fn save_tree(&self) -> Result<()> {
        let tree = self.tree.read().await;
        let tree_json = tree.to_json()?;

        let tree_key = self.master_key()?.derive_file_key(TREE_KEY_CONTEXT);
        let encrypted = encrypt(tree_key.as_bytes(), tree_json.as_bytes())
            .map_err(|e| Error::Crypto(format!("Failed to encrypt tree index: {}", e)))?;

        let tree_path = VaultPath::parse(META_DIRNAME)?.join(TREE_FILENAME)?;
        self.provider.upload(&tree_path, encrypted).await?;
        Ok(())
    }
}

impl Drop for VaultSession {
    fn drop(&mut self) {
        self.lock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axiomvault_storage::MemoryProvider;

    fn create_test_session() -> (VaultSession, VaultConfig) {
        let id = VaultId::new("test").unwrap();
        let password = b"test-password";
        let params = axiomvault_crypto::KdfParams::moderate();
        let creation =
            VaultConfig::new(id, password, "memory", serde_json::Value::Null, params).unwrap();
        let config = creation.config;

        let provider = Arc::new(MemoryProvider::new());
        let session =
            VaultSession::unlock(config.clone(), password, provider, VaultTree::new()).unwrap();

        (session, config)
    }

    #[test]
    fn test_session_creation() {
        let (session, _) = create_test_session();
        assert!(session.is_active());
        assert!(session.master_key().is_ok());
    }

    #[test]
    fn test_session_lock() {
        let (mut session, _) = create_test_session();
        session.lock();

        assert!(!session.is_active());
        assert_eq!(session.state(), SessionState::Locked);
        assert!(session.master_key().is_err());
    }

    #[test]
    fn test_wrong_password_fails() {
        let id = VaultId::new("test").unwrap();
        let password = b"correct";
        let params = axiomvault_crypto::KdfParams::moderate();
        let creation =
            VaultConfig::new(id, password, "memory", serde_json::Value::Null, params).unwrap();

        let provider = Arc::new(MemoryProvider::new());
        let result = VaultSession::unlock(creation.config, b"wrong", provider, VaultTree::new());

        assert!(result.is_err());
    }

    #[test]
    fn test_change_password() {
        let (mut session, _) = create_test_session();
        let old_password = b"test-password";
        let new_password = b"new-password";

        session.change_password(old_password, new_password).unwrap();

        assert!(session
            .config()
            .verify_password(new_password)
            .unwrap()
            .is_some());
        assert!(session
            .config()
            .verify_password(old_password)
            .unwrap()
            .is_none());
    }
}
