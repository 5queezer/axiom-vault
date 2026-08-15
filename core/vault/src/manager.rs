//! Vault manager for creating and managing vaults.

use std::sync::Arc;

use crate::config::{VaultConfig, CONFIG_FILENAME, DATA_DIRNAME, META_DIRNAME};
use crate::freshness::{FreshnessAnchor, LocalFileFreshnessAnchor, UnavailableFreshnessAnchor};
use crate::manifest::{GenerationManifest, MANIFEST_FILENAME};
use crate::session::VaultSession;
use crate::tree::VaultTree;
use axiomvault_common::{Error, Result, VaultId, VaultPath};
use axiomvault_crypto::recovery::RecoveryKey;
use axiomvault_crypto::KdfParams;
use axiomvault_storage::{create_default_registry, ProviderRegistry, StorageProvider};
use zeroize::Zeroizing;

/// Result of vault creation, containing the session and recovery words.
pub struct VaultCreation {
    /// Active session for the newly created vault.
    pub session: VaultSession,
    /// Recovery key encoded as 24 BIP39 words. Must be shown to user once.
    pub recovery_words: Zeroizing<String>,
}

/// Vault manager for creating and opening vaults.
pub struct VaultManager {
    registry: ProviderRegistry,
    freshness_anchor: Arc<dyn FreshnessAnchor>,
}

impl VaultManager {
    /// Create a new vault manager with default providers.
    pub fn new() -> Self {
        Self::with_registry(create_default_registry())
    }

    /// Create with custom registry.
    pub fn with_registry(registry: ProviderRegistry) -> Self {
        let freshness_anchor: Arc<dyn FreshnessAnchor> =
            match LocalFileFreshnessAnchor::platform_default() {
                Ok(anchor) => Arc::new(anchor),
                Err(error) => Arc::new(UnavailableFreshnessAnchor::new(error.to_string())),
            };
        Self::with_registry_and_anchor(registry, freshness_anchor)
    }

    /// Create with a custom provider registry and trusted freshness anchor.
    pub fn with_registry_and_anchor(
        registry: ProviderRegistry,
        freshness_anchor: Arc<dyn FreshnessAnchor>,
    ) -> Self {
        Self {
            registry,
            freshness_anchor,
        }
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
        if self.freshness_anchor.load(&vault_id)?.is_some() {
            return Err(Error::AlreadyExists(
                "freshness anchor already exists for vault ID".to_string(),
            ));
        }
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
        let session = VaultSession::from_master_key_with_freshness(
            creation.config,
            creation.master_key,
            provider,
            VaultTree::new(),
            self.freshness_anchor.clone(),
            0,
        )?;
        session.save_tree().await?;

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

        let manifest_path = VaultPath::parse(META_DIRNAME)?.join(MANIFEST_FILENAME)?;
        let anchored_generation = self.freshness_anchor.load(&config.id)?;

        if provider.exists(&manifest_path).await? {
            let tree_path = VaultPath::parse(META_DIRNAME)?.join(crate::config::TREE_FILENAME)?;
            if !provider.exists(&tree_path).await? {
                return Err(Error::Crypto(
                    "snapshot manifest exists but encrypted tree is missing".to_string(),
                ));
            }
            let encrypted_tree = provider.download(&tree_path).await?;
            let manifest_bytes = provider.download(&manifest_path).await?;
            let manifest = GenerationManifest::open(&master_key, &manifest_bytes)?;
            manifest.verify_bindings(&config.id, &encrypted_tree, &config_bytes)?;
            if anchored_generation.is_some_and(|accepted| manifest.generation < accepted) {
                return Err(Error::Conflict(format!(
                    "vault snapshot rollback detected: generation {} is older than trusted generation {}",
                    manifest.generation,
                    anchored_generation.unwrap_or_default()
                )));
            }
            let tree = VaultSession::decrypt_tree_bytes(&master_key, &encrypted_tree)?;
            self.freshness_anchor
                .store(&config.id, manifest.generation)?;
            return VaultSession::from_master_key_with_freshness(
                config,
                master_key,
                provider,
                tree,
                self.freshness_anchor.clone(),
                manifest.generation,
            );
        }

        if anchored_generation.is_some() {
            return Err(Error::Conflict(
                "snapshot manifest missing for a vault with trusted freshness state".to_string(),
            ));
        }

        // Legacy bootstrap is allowed only when neither storage nor this device has
        // freshness state. The authenticated generation is written before success.
        let tree = VaultSession::load_and_decrypt_tree(&provider, &master_key).await?;
        let session = VaultSession::from_master_key_with_freshness(
            config,
            master_key,
            provider,
            tree,
            self.freshness_anchor.clone(),
            0,
        )?;
        session.save_tree().await?;
        Ok(session)
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

        // Verify the currently stored authenticated snapshot before mutating config.
        let manifest_path = VaultPath::parse(META_DIRNAME)?.join(MANIFEST_FILENAME)?;
        let anchored_generation = self.freshness_anchor.load(&config.id)?;
        let (tree, generation) = if provider.exists(&manifest_path).await? {
            let tree_path = VaultPath::parse(META_DIRNAME)?.join(crate::config::TREE_FILENAME)?;
            if !provider.exists(&tree_path).await? {
                return Err(Error::Crypto(
                    "snapshot manifest exists but encrypted tree is missing".to_string(),
                ));
            }
            let encrypted_tree = provider.download(&tree_path).await?;
            let manifest_bytes = provider.download(&manifest_path).await?;
            let manifest = GenerationManifest::open(&master_key, &manifest_bytes)?;
            manifest.verify_bindings(&config.id, &encrypted_tree, &config_bytes)?;
            if anchored_generation.is_some_and(|accepted| manifest.generation < accepted) {
                return Err(Error::Conflict(
                    "vault snapshot rollback detected during recovery".to_string(),
                ));
            }
            let tree = VaultSession::decrypt_tree_bytes(&master_key, &encrypted_tree)?;
            self.freshness_anchor
                .store(&config.id, manifest.generation)?;
            (tree, manifest.generation)
        } else {
            if anchored_generation.is_some() {
                return Err(Error::Conflict(
                    "snapshot manifest missing for a vault with trusted freshness state"
                        .to_string(),
                ));
            }
            (
                VaultSession::load_and_decrypt_tree(&provider, &master_key).await?,
                0,
            )
        };

        // Reset password in config. The master key itself doesn't change.
        config.reset_password(&recovery_key, new_password)?;

        // Persist config first; the following snapshot commit binds it to the tree.
        let config_bytes = config.to_bytes()?;
        provider.upload(&config_path, config_bytes).await?;

        let session = VaultSession::from_master_key_with_freshness(
            config,
            master_key,
            provider,
            tree,
            self.freshness_anchor.clone(),
            generation,
        )?;
        session.save_tree().await?;
        Ok(session)
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
        // Config and tree are one authenticated snapshot generation. Re-sealing
        // the tree binds this exact config and advances the trusted anchor.
        session.save_tree().await?;
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
    use crate::freshness::{FreshnessAnchor, InMemoryFreshnessAnchor};
    use crate::manifest::MANIFEST_FILENAME;
    use axiomvault_storage::MemoryProvider;

    fn manager_with_storage(
        provider: Arc<MemoryProvider>,
        anchor: Arc<dyn FreshnessAnchor>,
    ) -> VaultManager {
        let mut registry = ProviderRegistry::new();
        registry
            .register("test", Box::new(move |_| Ok(provider.clone())))
            .unwrap();
        VaultManager::with_registry_and_anchor(registry, anchor)
    }

    fn tree_path() -> VaultPath {
        VaultPath::parse(META_DIRNAME)
            .unwrap()
            .join(crate::config::TREE_FILENAME)
            .unwrap()
    }

    fn manifest_path() -> VaultPath {
        VaultPath::parse(META_DIRNAME)
            .unwrap()
            .join(MANIFEST_FILENAME)
            .unwrap()
    }

    async fn stored_snapshot(provider: &MemoryProvider) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        (
            provider
                .download(&VaultPath::parse(CONFIG_FILENAME).unwrap())
                .await
                .unwrap(),
            provider.download(&tree_path()).await.unwrap(),
            provider.download(&manifest_path()).await.unwrap(),
        )
    }

    struct FailingAnchor;

    impl FreshnessAnchor for FailingAnchor {
        fn load(&self, _vault_id: &VaultId) -> Result<Option<u64>> {
            Err(Error::Vault("anchor I/O failed".to_string()))
        }

        fn store(&self, _vault_id: &VaultId, _generation: u64) -> Result<()> {
            Err(Error::Vault("anchor I/O failed".to_string()))
        }
    }

    #[tokio::test]
    async fn anchor_read_errors_fail_closed_before_create() {
        let provider = Arc::new(MemoryProvider::new());
        let manager = manager_with_storage(provider.clone(), Arc::new(FailingAnchor));
        let result = manager
            .create_vault(
                VaultId::new("anchor-error").unwrap(),
                b"password",
                "test",
                serde_json::Value::Null,
                KdfParams::moderate(),
            )
            .await;

        assert!(result.is_err());
        assert!(!provider
            .exists(&VaultPath::parse(CONFIG_FILENAME).unwrap())
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn removing_manifest_after_anchor_exists_is_rejected() {
        let provider = Arc::new(MemoryProvider::new());
        let manager =
            manager_with_storage(provider.clone(), Arc::new(InMemoryFreshnessAnchor::new()));
        manager
            .create_vault(
                VaultId::new("missing-manifest").unwrap(),
                b"password",
                "test",
                serde_json::Value::Null,
                KdfParams::moderate(),
            )
            .await
            .unwrap();
        provider.delete(&manifest_path()).await.unwrap();

        let error = manager
            .open_vault("test", serde_json::Value::Null, b"password")
            .await
            .err()
            .expect("missing manifest must fail closed");
        assert!(error.to_string().contains("manifest missing"));
    }

    #[tokio::test]
    async fn create_persists_authenticated_generation_and_anchor() {
        let provider = Arc::new(MemoryProvider::new());
        let anchor = Arc::new(InMemoryFreshnessAnchor::new());
        let manager = manager_with_storage(provider.clone(), anchor.clone());
        let vault_id = VaultId::new("fresh-vault").unwrap();

        manager
            .create_vault(
                vault_id.clone(),
                b"password",
                "test",
                serde_json::Value::Null,
                KdfParams::moderate(),
            )
            .await
            .unwrap();

        let manifest_path = VaultPath::parse(META_DIRNAME)
            .unwrap()
            .join(MANIFEST_FILENAME)
            .unwrap();
        assert!(provider.exists(&manifest_path).await.unwrap());
        assert_eq!(anchor.load(&vault_id).unwrap(), Some(1));
    }

    #[tokio::test]
    async fn open_restores_generation_for_the_next_update() {
        let provider = Arc::new(MemoryProvider::new());
        let anchor = Arc::new(InMemoryFreshnessAnchor::new());
        let manager = manager_with_storage(provider, anchor.clone());
        let vault_id = VaultId::new("reopen-update").unwrap();
        let creation = manager
            .create_vault(
                vault_id.clone(),
                b"password",
                "test",
                serde_json::Value::Null,
                KdfParams::moderate(),
            )
            .await
            .unwrap();
        drop(creation);

        let reopened = manager
            .open_vault("test", serde_json::Value::Null, b"password")
            .await
            .unwrap();
        reopened.save_tree().await.unwrap();

        assert_eq!(anchor.load(&vault_id).unwrap(), Some(2));
    }

    #[tokio::test]
    async fn replaying_an_older_file_ciphertext_is_rejected() {
        use crate::operations::VaultOperations;

        let provider = Arc::new(MemoryProvider::new());
        let anchor = Arc::new(InMemoryFreshnessAnchor::new());
        let manager = manager_with_storage(provider.clone(), anchor);
        let creation = manager
            .create_vault(
                VaultId::new("file-rollback").unwrap(),
                b"password",
                "test",
                serde_json::Value::Null,
                KdfParams::moderate(),
            )
            .await
            .unwrap();
        let ops = VaultOperations::new(&creation.session).unwrap();
        let path = VaultPath::parse("/note.txt").unwrap();
        ops.create_file(&path, b"generation one").await.unwrap();
        let encrypted_name = creation
            .session
            .tree()
            .read()
            .await
            .get_node(&path)
            .unwrap()
            .metadata
            .encrypted_name
            .clone();
        let object_path = VaultPath::parse(DATA_DIRNAME)
            .unwrap()
            .join(&encrypted_name)
            .unwrap();
        let old_ciphertext = provider.download(&object_path).await.unwrap();

        ops.update_file(&path, b"generation two").await.unwrap();
        provider.upload(&object_path, old_ciphertext).await.unwrap();

        assert!(ops.read_file(&path).await.is_err());
    }

    #[tokio::test]
    async fn password_change_commits_a_new_authenticated_generation() {
        let provider = Arc::new(MemoryProvider::new());
        let anchor = Arc::new(InMemoryFreshnessAnchor::new());
        let manager = manager_with_storage(provider, anchor.clone());
        let vault_id = VaultId::new("password-generation").unwrap();
        let mut creation = manager
            .create_vault(
                vault_id.clone(),
                b"old-password",
                "test",
                serde_json::Value::Null,
                KdfParams::moderate(),
            )
            .await
            .unwrap();

        creation
            .session
            .change_password(b"old-password", b"new-password")
            .unwrap();
        manager.save_config(&creation.session).await.unwrap();
        drop(creation);

        let reopened = manager
            .open_vault("test", serde_json::Value::Null, b"new-password")
            .await
            .unwrap();
        assert!(reopened.is_active());
        assert_eq!(anchor.load(&vault_id).unwrap(), Some(2));
    }

    #[tokio::test]
    async fn recovery_password_reset_commits_a_fresh_generation() {
        let provider = Arc::new(MemoryProvider::new());
        let anchor = Arc::new(InMemoryFreshnessAnchor::new());
        let manager = manager_with_storage(provider, anchor.clone());
        let vault_id = VaultId::new("recovery-generation").unwrap();
        let creation = manager
            .create_vault(
                vault_id.clone(),
                b"old-password",
                "test",
                serde_json::Value::Null,
                KdfParams::moderate(),
            )
            .await
            .unwrap();
        let words = creation.recovery_words.clone();
        drop(creation);

        manager
            .recover_vault("test", serde_json::Value::Null, &words, b"new-password")
            .await
            .unwrap();
        manager
            .open_vault("test", serde_json::Value::Null, b"new-password")
            .await
            .unwrap();
        assert_eq!(anchor.load(&vault_id).unwrap(), Some(2));
    }

    #[tokio::test]
    async fn full_snapshot_rollback_is_rejected() {
        let provider = Arc::new(MemoryProvider::new());
        let anchor = Arc::new(InMemoryFreshnessAnchor::new());
        let manager = manager_with_storage(provider.clone(), anchor);
        let creation = manager
            .create_vault(
                VaultId::new("full-rollback").unwrap(),
                b"password",
                "test",
                serde_json::Value::Null,
                KdfParams::moderate(),
            )
            .await
            .unwrap();
        let old = stored_snapshot(&provider).await;
        creation.session.save_tree().await.unwrap();
        provider
            .upload(&VaultPath::parse(CONFIG_FILENAME).unwrap(), old.0)
            .await
            .unwrap();
        provider.upload(&tree_path(), old.1).await.unwrap();
        provider.upload(&manifest_path(), old.2).await.unwrap();

        let error = manager
            .open_vault("test", serde_json::Value::Null, b"password")
            .await
            .err()
            .expect("rollback must be rejected");
        assert!(error.to_string().contains("rollback detected"));
    }

    #[tokio::test]
    async fn mixed_tree_and_manifest_generations_are_rejected() {
        let provider = Arc::new(MemoryProvider::new());
        let manager =
            manager_with_storage(provider.clone(), Arc::new(InMemoryFreshnessAnchor::new()));
        let creation = manager
            .create_vault(
                VaultId::new("mixed-generation").unwrap(),
                b"password",
                "test",
                serde_json::Value::Null,
                KdfParams::moderate(),
            )
            .await
            .unwrap();
        let old_tree = provider.download(&tree_path()).await.unwrap();
        creation.session.save_tree().await.unwrap();
        provider.upload(&tree_path(), old_tree).await.unwrap();

        let error = manager
            .open_vault("test", serde_json::Value::Null, b"password")
            .await
            .err()
            .expect("rollback must be rejected");
        assert!(error.to_string().contains("tree and manifest"));
    }

    #[tokio::test]
    async fn tampered_manifest_is_rejected() {
        let provider = Arc::new(MemoryProvider::new());
        let manager =
            manager_with_storage(provider.clone(), Arc::new(InMemoryFreshnessAnchor::new()));
        manager
            .create_vault(
                VaultId::new("manifest-tamper").unwrap(),
                b"password",
                "test",
                serde_json::Value::Null,
                KdfParams::moderate(),
            )
            .await
            .unwrap();
        let mut bytes = provider.download(&manifest_path()).await.unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        provider.upload(&manifest_path(), bytes).await.unwrap();

        let error = manager
            .open_vault("test", serde_json::Value::Null, b"password")
            .await
            .err()
            .expect("rollback must be rejected");
        assert!(error.to_string().contains("authentication failed"));
    }

    #[tokio::test]
    async fn pre_password_change_config_replay_is_rejected() {
        let provider = Arc::new(MemoryProvider::new());
        let manager =
            manager_with_storage(provider.clone(), Arc::new(InMemoryFreshnessAnchor::new()));
        let mut creation = manager
            .create_vault(
                VaultId::new("config-rollback").unwrap(),
                b"old-password",
                "test",
                serde_json::Value::Null,
                KdfParams::moderate(),
            )
            .await
            .unwrap();
        let old_config = provider
            .download(&VaultPath::parse(CONFIG_FILENAME).unwrap())
            .await
            .unwrap();
        creation
            .session
            .change_password(b"old-password", b"new-password")
            .unwrap();
        manager.save_config(&creation.session).await.unwrap();
        provider
            .upload(&VaultPath::parse(CONFIG_FILENAME).unwrap(), old_config)
            .await
            .unwrap();

        let error = manager
            .open_vault("test", serde_json::Value::Null, b"old-password")
            .await
            .err()
            .expect("rollback must be rejected");
        assert!(error.to_string().contains("config and snapshot manifest"));
    }

    #[tokio::test]
    async fn authenticated_first_open_bootstraps_a_new_local_anchor() {
        let provider = Arc::new(MemoryProvider::new());
        let creator =
            manager_with_storage(provider.clone(), Arc::new(InMemoryFreshnessAnchor::new()));
        creator
            .create_vault(
                VaultId::new("first-open").unwrap(),
                b"password",
                "test",
                serde_json::Value::Null,
                KdfParams::moderate(),
            )
            .await
            .unwrap();

        let new_anchor = Arc::new(InMemoryFreshnessAnchor::new());
        let opener = manager_with_storage(provider, new_anchor.clone());
        opener
            .open_vault("test", serde_json::Value::Null, b"password")
            .await
            .unwrap();
        assert_eq!(
            new_anchor
                .load(&VaultId::new("first-open").unwrap())
                .unwrap(),
            Some(1)
        );
    }

    #[tokio::test]
    async fn legacy_vault_without_any_anchor_is_bootstrapped() {
        let provider = Arc::new(MemoryProvider::new());
        let anchor = Arc::new(InMemoryFreshnessAnchor::new());
        let manager = manager_with_storage(provider.clone(), anchor.clone());
        let creation = VaultConfig::new(
            VaultId::new("legacy-bootstrap").unwrap(),
            b"password",
            "test",
            serde_json::Value::Null,
            KdfParams::moderate(),
        )
        .unwrap();
        let storage: Arc<dyn StorageProvider> = provider.clone();
        manager
            .initialize_vault_structure(&storage, &creation.config)
            .await
            .unwrap();
        let legacy_session = VaultSession::from_master_key(
            creation.config,
            creation.master_key,
            provider.clone(),
            VaultTree::new(),
        )
        .unwrap();
        legacy_session.save_tree().await.unwrap();
        assert!(!provider.exists(&manifest_path()).await.unwrap());

        manager
            .open_vault("test", serde_json::Value::Null, b"password")
            .await
            .unwrap();
        assert!(provider.exists(&manifest_path()).await.unwrap());
        assert_eq!(
            anchor
                .load(&VaultId::new("legacy-bootstrap").unwrap())
                .unwrap(),
            Some(1)
        );
    }

    #[tokio::test]
    async fn test_create_vault() {
        let manager = VaultManager::with_registry_and_anchor(
            create_default_registry(),
            Arc::new(InMemoryFreshnessAnchor::new()),
        );
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
        let manager = VaultManager::with_registry_and_anchor(
            create_default_registry(),
            Arc::new(InMemoryFreshnessAnchor::new()),
        );
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
        let manager = VaultManager::with_registry_and_anchor(
            create_default_registry(),
            Arc::new(InMemoryFreshnessAnchor::new()),
        );

        let exists = manager
            .vault_exists("memory", serde_json::Value::Null)
            .await;
        assert!(exists.is_ok());
    }
}
