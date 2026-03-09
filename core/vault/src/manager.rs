//! Vault manager for creating and managing vaults.

use std::sync::Arc;

use crate::config::{VaultConfig, CONFIG_FILENAME, DATA_DIRNAME, META_DIRNAME};
use crate::session::VaultSession;
use crate::tree::VaultTree;
use axiomvault_common::{Error, Result, VaultId, VaultPath};
use axiomvault_crypto::recovery::RecoveryKey;
use axiomvault_crypto::KdfParams;
use axiomvault_storage::{create_default_registry, ProviderRegistry, StorageProvider};

/// Result of vault creation, containing the session and recovery words.
pub struct VaultCreation {
    /// Active session for the newly created vault.
    pub session: VaultSession,
    /// Recovery key encoded as 24 BIP39 words. Must be shown to user once.
    pub recovery_words: String,
}

/// Vault manager for creating and opening vaults.
pub struct VaultManager {
    registry: ProviderRegistry,
}

impl VaultManager {
    /// Create a new vault manager with default providers.
    pub fn new() -> Self {
        Self {
            registry: create_default_registry(),
        }
    }

    /// Create with custom registry.
    pub fn with_registry(registry: ProviderRegistry) -> Self {
        Self { registry }
    }

    /// Get the provider registry.
    pub fn registry(&self) -> &ProviderRegistry {
        &self.registry
    }

    /// Get mutable provider registry.
    pub fn registry_mut(&mut self) -> &mut ProviderRegistry {
        &mut self.registry
    }

    /// Create a new vault.
    ///
    /// # Returns
    /// A `VaultCreation` with the active session and recovery words.
    pub async fn create_vault(
        &self,
        vault_id: VaultId,
        password: &[u8],
        provider_type: &str,
        provider_config: serde_json::Value,
        kdf_params: KdfParams,
    ) -> Result<VaultCreation> {
        let provider = self
            .registry
            .resolve(provider_type, provider_config.clone())?;

        let creation = VaultConfig::new(
            vault_id,
            password,
            provider_type,
            provider_config,
            kdf_params,
        )?;

        self.initialize_vault_structure(&provider, &creation.config)
            .await?;

        // Use from_master_key to avoid a second Argon2id KDF round.
        let session = VaultSession::from_master_key(
            creation.config,
            creation.master_key,
            provider,
            VaultTree::new(),
        )?;

        Ok(VaultCreation {
            session,
            recovery_words: creation.recovery_words,
        })
    }

    /// Initialize vault directory structure.
    async fn initialize_vault_structure(
        &self,
        provider: &Arc<dyn StorageProvider>,
        config: &VaultConfig,
    ) -> Result<()> {
        let data_path = VaultPath::parse(DATA_DIRNAME)?;
        if !provider.exists(&data_path).await? {
            provider.create_dir(&data_path).await?;
        }

        let meta_path = VaultPath::parse(META_DIRNAME)?;
        if !provider.exists(&meta_path).await? {
            provider.create_dir(&meta_path).await?;
        }

        let config_path = VaultPath::parse(CONFIG_FILENAME)?;
        let config_bytes = config.to_bytes()?;
        provider.upload(&config_path, config_bytes).await?;

        Ok(())
    }

    /// Open an existing vault.
    pub async fn open_vault(
        &self,
        provider_type: &str,
        provider_config: serde_json::Value,
        password: &[u8],
    ) -> Result<VaultSession> {
        let provider = self.registry.resolve(provider_type, provider_config)?;

        let config_path = VaultPath::parse(CONFIG_FILENAME)?;
        if !provider.exists(&config_path).await? {
            return Err(Error::NotFound("Vault configuration not found".to_string()));
        }

        let config_bytes = provider.download(&config_path).await?;
        let config = VaultConfig::from_bytes(&config_bytes)?;

        let master_key = config
            .verify_password(password)?
            .ok_or_else(|| Error::NotPermitted("Invalid password".to_string()))?;

        let tree = VaultSession::load_and_decrypt_tree(&provider, &master_key).await?;

        VaultSession::from_master_key(config, master_key, provider, tree)
    }

    /// Reset vault password using recovery key words.
    ///
    /// # Postconditions
    /// - Vault password is changed to new_password
    /// - Recovery key is unchanged
    /// - Returns an active session
    pub async fn recover_vault(
        &self,
        provider_type: &str,
        provider_config: serde_json::Value,
        recovery_words: &str,
        new_password: &[u8],
    ) -> Result<VaultSession> {
        let provider = self.registry.resolve(provider_type, provider_config)?;

        let config_path = VaultPath::parse(CONFIG_FILENAME)?;
        if !provider.exists(&config_path).await? {
            return Err(Error::NotFound("Vault configuration not found".to_string()));
        }

        let config_bytes = provider.download(&config_path).await?;
        let mut config = VaultConfig::from_bytes(&config_bytes)?;

        let recovery_key = RecoveryKey::from_mnemonic(recovery_words)?;

        // Verify recovery key and get master key for tree decryption.
        let master_key = config
            .verify_recovery_key(&recovery_key)?
            .ok_or_else(|| Error::NotPermitted("Invalid recovery key".to_string()))?;

        // Load the tree with the master key before resetting the password.
        let tree = VaultSession::load_and_decrypt_tree(&provider, &master_key).await?;

        // Reset password in config. The master key itself doesn't change.
        config.reset_password(&recovery_key, new_password)?;

        // Save updated config.
        let config_bytes = config.to_bytes()?;
        provider.upload(&config_path, config_bytes).await?;

        // Reuse the master key from recovery — no need for a second Argon2id round.
        VaultSession::from_master_key(config, master_key, provider, tree)
    }

    /// Check if a vault exists at the given location.
    pub async fn vault_exists(
        &self,
        provider_type: &str,
        provider_config: serde_json::Value,
    ) -> Result<bool> {
        let provider = self.registry.resolve(provider_type, provider_config)?;
        let config_path = VaultPath::parse(CONFIG_FILENAME)?;
        provider.exists(&config_path).await
    }

    /// Save vault configuration to storage.
    pub async fn save_config(&self, session: &VaultSession) -> Result<()> {
        let config_path = VaultPath::parse(CONFIG_FILENAME)?;
        let config_bytes = session.config().to_bytes()?;
        session
            .provider()
            .upload(&config_path, config_bytes)
            .await?;
        Ok(())
    }

    /// Save vault tree to storage (encrypted).
    pub async fn save_tree(&self, session: &VaultSession) -> Result<()> {
        session.save_tree().await
    }
}

impl Default for VaultManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_vault() {
        let manager = VaultManager::new();
        let vault_id = VaultId::new("test-vault").unwrap();
        let password = b"secure-password";

        let creation = manager
            .create_vault(
                vault_id.clone(),
                password,
                "memory",
                serde_json::Value::Null,
                KdfParams::moderate(),
            )
            .await
            .unwrap();

        assert!(creation.session.is_active());
        assert_eq!(creation.session.vault_id().as_str(), vault_id.as_str());
        assert_eq!(creation.recovery_words.split_whitespace().count(), 24);
    }

    #[tokio::test]
    async fn test_open_vault() {
        let manager = VaultManager::new();
        let vault_id = VaultId::new("test-vault").unwrap();
        let password = b"secure-password";

        let creation = manager
            .create_vault(
                vault_id.clone(),
                password,
                "memory",
                serde_json::Value::Null,
                KdfParams::moderate(),
            )
            .await
            .unwrap();

        let provider = creation.session.provider();
        drop(creation.session);

        let config_path = VaultPath::parse(CONFIG_FILENAME).unwrap();
        let config_bytes = provider.download(&config_path).await.unwrap();
        let config = VaultConfig::from_bytes(&config_bytes).unwrap();

        let master_key = config
            .verify_password(password)
            .unwrap()
            .expect("password should be correct");

        let tree = VaultSession::load_and_decrypt_tree(&provider, &master_key)
            .await
            .unwrap();

        let reopened = VaultSession::from_master_key(config, master_key, provider, tree).unwrap();
        assert!(reopened.is_active());
        assert_eq!(reopened.vault_id().as_str(), vault_id.as_str());
    }

    #[tokio::test]
    async fn test_vault_exists() {
        let manager = VaultManager::new();

        let exists = manager
            .vault_exists("memory", serde_json::Value::Null)
            .await;
        assert!(exists.is_ok());
    }
}
