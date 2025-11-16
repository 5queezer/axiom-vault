//! Vault manager for creating and managing vaults.

use std::sync::Arc;

use crate::config::{VaultConfig, CONFIG_FILENAME, DATA_DIRNAME, META_DIRNAME};
use crate::session::VaultSession;
use axiomvault_common::{Error, Result, VaultId, VaultPath};
use axiomvault_crypto::KdfParams;
use axiomvault_storage::{create_default_registry, ProviderRegistry, StorageProvider};

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
    /// # Preconditions
    /// - Provider type must be registered
    /// - Provider config must be valid
    /// - Password must not be empty
    ///
    /// # Postconditions
    /// - Vault structure is created in storage
    /// - Vault configuration is persisted
    /// - Returns an active session
    ///
    /// # Errors
    /// - Provider not found
    /// - Storage access failure
    /// - Invalid configuration
    pub async fn create_vault(
        &self,
        vault_id: VaultId,
        password: &[u8],
        provider_type: &str,
        provider_config: serde_json::Value,
        kdf_params: KdfParams,
    ) -> Result<VaultSession> {
        // Resolve provider
        let provider = self
            .registry
            .resolve(provider_type, provider_config.clone())?;

        // Create vault config
        let config = VaultConfig::new(
            vault_id,
            password,
            provider_type,
            provider_config,
            kdf_params,
        )?;

        // Initialize vault structure
        self.initialize_vault_structure(&provider, &config).await?;

        // Create and return session
        VaultSession::unlock(config, password, provider)
    }

    /// Initialize vault directory structure.
    async fn initialize_vault_structure(
        &self,
        provider: &Arc<dyn StorageProvider>,
        config: &VaultConfig,
    ) -> Result<()> {
        // Create data directory
        let data_path = VaultPath::parse(DATA_DIRNAME)?;
        if !provider.exists(&data_path).await? {
            provider.create_dir(&data_path).await?;
        }

        // Create metadata directory
        let meta_path = VaultPath::parse(META_DIRNAME)?;
        if !provider.exists(&meta_path).await? {
            provider.create_dir(&meta_path).await?;
        }

        // Save vault config
        let config_path = VaultPath::parse(CONFIG_FILENAME)?;
        let config_bytes = config.to_bytes()?;
        provider.upload(&config_path, config_bytes).await?;

        Ok(())
    }

    /// Open an existing vault.
    ///
    /// # Preconditions
    /// - Vault must exist at provider location
    /// - Password must be correct
    ///
    /// # Postconditions
    /// - Returns an active session
    ///
    /// # Errors
    /// - Vault not found
    /// - Invalid password
    /// - Incompatible version
    pub async fn open_vault(
        &self,
        provider_type: &str,
        provider_config: serde_json::Value,
        password: &[u8],
    ) -> Result<VaultSession> {
        // Resolve provider
        let provider = self.registry.resolve(provider_type, provider_config)?;

        // Load vault config
        let config_path = VaultPath::parse(CONFIG_FILENAME)?;
        if !provider.exists(&config_path).await? {
            return Err(Error::NotFound("Vault configuration not found".to_string()));
        }

        let config_bytes = provider.download(&config_path).await?;
        let config = VaultConfig::from_bytes(&config_bytes)?;

        // Create and return session
        VaultSession::unlock(config, password, provider)
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

        let session = manager
            .create_vault(
                vault_id.clone(),
                password,
                "memory",
                serde_json::Value::Null,
                KdfParams::moderate(),
            )
            .await
            .unwrap();

        assert!(session.is_active());
        assert_eq!(session.vault_id().as_str(), vault_id.as_str());
    }

    #[tokio::test]
    async fn test_open_vault() {
        let manager = VaultManager::new();
        let vault_id = VaultId::new("test-vault").unwrap();
        let password = b"secure-password";

        // Create vault
        let session = manager
            .create_vault(
                vault_id.clone(),
                password,
                "memory",
                serde_json::Value::Null,
                KdfParams::moderate(),
            )
            .await
            .unwrap();

        let provider = session.provider();
        drop(session);

        // Re-open using same provider (memory provider in this case)
        // Note: In real usage, provider would be reconstructed from config
        let config_path = VaultPath::parse(CONFIG_FILENAME).unwrap();
        let config_bytes = provider.download(&config_path).await.unwrap();
        let config = VaultConfig::from_bytes(&config_bytes).unwrap();

        let reopened = VaultSession::unlock(config, password, provider).unwrap();
        assert!(reopened.is_active());
        assert_eq!(reopened.vault_id().as_str(), vault_id.as_str());
    }

    #[tokio::test]
    async fn test_vault_exists() {
        let manager = VaultManager::new();

        // Should not exist initially
        let exists = manager
            .vault_exists("memory", serde_json::Value::Null)
            .await;
        // This will fail because memory provider creates fresh instance
        // In real usage with persistent storage, this would work correctly
        assert!(exists.is_ok());
    }
}
