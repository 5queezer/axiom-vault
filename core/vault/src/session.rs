//! Vault session management.
//!
//! Sessions hold decrypted keys in memory and provide access to vault operations.
//! Keys are automatically zeroized when the session is dropped.

use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use axiomvault_common::{Result, Error, VaultId};
use axiomvault_crypto::{MasterKey, derive_key};
use axiomvault_storage::StorageProvider;
use crate::config::VaultConfig;
use crate::tree::VaultTree;

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
    /// Create a new vault session by unlocking with password.
    ///
    /// # Preconditions
    /// - Config must be valid
    /// - Password must be correct
    /// - Provider must be connected
    ///
    /// # Postconditions
    /// - Returns active session with decrypted master key
    /// - Session handle is unique
    ///
    /// # Errors
    /// - Invalid password
    /// - Incompatible vault version
    /// - Storage access failure
    pub fn unlock(
        config: VaultConfig,
        password: &[u8],
        provider: Arc<dyn StorageProvider>,
    ) -> Result<Self> {
        // Verify vault version
        if !config.version.is_compatible() {
            return Err(Error::Vault(format!(
                "Incompatible vault version: {:?}",
                config.version
            )));
        }

        // Verify password
        if !config.verify_password(password)? {
            return Err(Error::NotPermitted("Invalid password".to_string()));
        }

        // Derive master key
        let master_key = derive_key(password, &config.salt, &config.kdf_params)?;

        let tree = Arc::new(RwLock::new(VaultTree::new()));

        Ok(Self {
            handle: SessionHandle::new(),
            config,
            master_key: Some(master_key),
            provider,
            tree,
            state: SessionState::Active,
        })
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

    /// Get the storage provider.
    pub fn provider(&self) -> Arc<dyn StorageProvider> {
        self.provider.clone()
    }

    /// Get reference to the vault tree.
    pub fn tree(&self) -> &Arc<RwLock<VaultTree>> {
        &self.tree
    }

    /// Get the master key, if session is active.
    ///
    /// # Errors
    /// - Returns error if session is locked
    pub fn master_key(&self) -> Result<&MasterKey> {
        match self.state {
            SessionState::Active => self
                .master_key
                .as_ref()
                .ok_or_else(|| Error::Vault("Master key not available".to_string())),
            SessionState::Locked => {
                Err(Error::NotPermitted("Session is locked".to_string()))
            }
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
    ///
    /// # Postconditions
    /// - Master key is zeroized and removed
    /// - Session state is Locked
    /// - Session can no longer perform operations
    pub fn lock(&mut self) {
        if let Some(key) = self.master_key.take() {
            // The key will be zeroized on drop due to ZeroizeOnDrop
            drop(key);
        }
        self.state = SessionState::Locked;
    }

    /// Relock the vault with a new password.
    ///
    /// # Preconditions
    /// - Session must be active
    /// - Old password must be correct
    /// - New password must not be empty
    ///
    /// # Postconditions
    /// - Master key is derived from new password
    /// - Config is updated with new salt and verification
    ///
    /// # Errors
    /// - Session is locked
    /// - Old password incorrect
    /// - New password empty
    pub fn change_password(
        &mut self,
        old_password: &[u8],
        new_password: &[u8],
    ) -> Result<()> {
        if self.state != SessionState::Active {
            return Err(Error::NotPermitted("Session is locked".to_string()));
        }

        // Verify old password
        if !self.config.verify_password(old_password)? {
            return Err(Error::NotPermitted("Invalid old password".to_string()));
        }

        // Create new config with new password
        let new_config = VaultConfig::new(
            self.config.id.clone(),
            new_password,
            self.config.provider_type.clone(),
            self.config.provider_config.clone(),
            self.config.kdf_params.clone(),
        )?;

        // Derive new master key
        let new_master_key = derive_key(new_password, &new_config.salt, &new_config.kdf_params)?;

        // Update session
        self.config = new_config;
        self.master_key = Some(new_master_key);

        Ok(())
    }
}

impl Drop for VaultSession {
    fn drop(&mut self) {
        // Ensure keys are zeroized
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
        let config = VaultConfig::new(
            id,
            password,
            "memory",
            serde_json::Value::Null,
            params,
        ).unwrap();

        let provider = Arc::new(MemoryProvider::new());
        let session = VaultSession::unlock(config.clone(), password, provider).unwrap();

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
        let config = VaultConfig::new(
            id,
            password,
            "memory",
            serde_json::Value::Null,
            params,
        ).unwrap();

        let provider = Arc::new(MemoryProvider::new());
        let result = VaultSession::unlock(config, b"wrong", provider);

        assert!(result.is_err());
    }

    #[test]
    fn test_change_password() {
        let (mut session, _) = create_test_session();
        let old_password = b"test-password";
        let new_password = b"new-password";

        session.change_password(old_password, new_password).unwrap();

        // Verify new password works
        assert!(session.config().verify_password(new_password).unwrap());
        // Old password should not work
        assert!(!session.config().verify_password(old_password).unwrap());
    }
}
