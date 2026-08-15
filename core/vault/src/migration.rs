//! Vault format migration framework.
//!
//! Provides versioned migrations for upgrading vault formats between versions.
//! Migrations are executed in sequence with automatic backup and rollback support.

use std::fmt;

use tracing::{info, warn};
use zeroize::Zeroizing;

use crate::config::{VaultConfig, VaultVersion, CONFIG_FILENAME};
use axiomvault_common::{Error, Result, VaultPath};
use axiomvault_crypto::recovery::RecoveryKey;
use axiomvault_storage::StorageProvider;

/// Backup filename for vault config during migration.
pub const CONFIG_BACKUP_FILENAME: &str = "vault.config.backup";
const CONFIG_MIGRATION_FILENAME: &str = "vault.config.migration";

/// Fully verified result of a committed migration.
pub struct MigrationOutcome {
    pub config: VaultConfig,
    pub recovery_words: Option<Zeroizing<String>>,
}

/// Status of migration check for a vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationStatus {
    /// Vault is at the current version; no migration needed.
    UpToDate,
    /// Vault needs migration from one version to another.
    NeedsMigration {
        from: VaultVersion,
        to: VaultVersion,
    },
    /// Vault version is incompatible (different major version, no migration path).
    Incompatible { version: VaultVersion },
}

impl fmt::Display for MigrationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MigrationStatus::UpToDate => write!(f, "Up to date"),
            MigrationStatus::NeedsMigration { from, to } => {
                write!(f, "Needs migration from {} to {}", from, to)
            }
            MigrationStatus::Incompatible { version } => {
                write!(f, "Incompatible version: {}", version)
            }
        }
    }
}

/// A single migration step from one version to the next.
pub trait Migration: Send + Sync {
    /// The version this migration upgrades from.
    fn source_version(&self) -> VaultVersion;
    /// The version this migration upgrades to.
    fn target_version(&self) -> VaultVersion;
    /// Human-readable description of what this migration does.
    fn description(&self) -> &str;
    /// Transform a config using authenticated secret material.
    fn migrate(
        &self,
        config: &mut VaultConfig,
        password: &[u8],
    ) -> Result<Option<Zeroizing<String>>>;
}

/// Registry of all available migrations.
pub struct MigrationRegistry {
    migrations: Vec<Box<dyn Migration>>,
}

impl MigrationRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            migrations: Vec::new(),
        }
    }

    /// Create a registry pre-populated with all known migrations.
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(MigrationV1_0ToV1_1));
        registry
    }

    /// Register a migration step.
    pub fn register(&mut self, migration: Box<dyn Migration>) {
        self.migrations.push(migration);
    }

    /// Find the ordered migration path from one version to another.
    ///
    /// Returns `None` if no path exists (e.g., incompatible major versions).
    pub fn find_path(&self, from: &VaultVersion, to: &VaultVersion) -> Option<Vec<&dyn Migration>> {
        if from == to {
            return Some(Vec::new());
        }

        // Only support forward migration within the same major version.
        if from.major != to.major {
            return None;
        }

        // Forward migration: from.minor < to.minor
        if from.minor >= to.minor {
            return None;
        }

        let mut path = Vec::new();
        let mut current = *from;

        while current != *to {
            let migration = self
                .migrations
                .iter()
                .find(|m| m.source_version() == current)?;
            let target = migration.target_version();
            // Ensure we're moving forward and not past our target.
            if target.minor > to.minor || target.major != to.major {
                return None;
            }
            path.push(migration.as_ref());
            current = target;
        }

        Some(path)
    }

    /// Authenticate, transform, verify, and atomically commit all migrations.
    ///
    /// The config is loaded and persisted exclusively through `StorageProvider`,
    /// so local and cloud-backed vaults share identical rollback semantics.
    pub async fn migrate(
        &self,
        provider: &dyn StorageProvider,
        password: &[u8],
        target: &VaultVersion,
    ) -> Result<MigrationOutcome> {
        let config_path = VaultPath::parse(CONFIG_FILENAME)?;
        let backup_path = VaultPath::parse(CONFIG_BACKUP_FILENAME)?;
        let temp_path = VaultPath::parse(CONFIG_MIGRATION_FILENAME)?;

        self.recover_interrupted(provider, &config_path, &backup_path, &temp_path)
            .await?;

        let original_bytes = provider.download(&config_path).await?;
        let mut config = VaultConfig::from_bytes(&original_bytes)?;
        let from = config.version;
        let path = self.find_path(&from, target).ok_or_else(|| {
            Error::Vault(format!("No migration path from {} to {}", from, target))
        })?;
        if path.is_empty() {
            validate_v1_1_invariants(&config)?;
            return Ok(MigrationOutcome {
                config,
                recovery_words: None,
            });
        }

        let original_master_key = config
            .verify_password(password)?
            .ok_or_else(|| Error::NotPermitted("Invalid password".to_string()))?;
        let mut recovery_words = None;
        for migration in path {
            recovery_words = migration.migrate(&mut config, password)?;
        }
        if config.version != *target {
            return Err(Error::Vault(format!(
                "Migration completed in memory at {} instead of {}",
                config.version, target
            )));
        }
        validate_v1_1_invariants(&config)?;
        let password_master_key = config
            .verify_password(password)?
            .ok_or_else(|| Error::Vault("Migrated config rejected its password".to_string()))?;
        if password_master_key.as_bytes() != original_master_key.as_bytes() {
            return Err(Error::Vault(
                "Migration changed the vault master key".to_string(),
            ));
        }
        let words = recovery_words.as_ref().ok_or_else(|| {
            Error::Vault("Migration did not produce required recovery words".to_string())
        })?;
        let recovery_key = RecoveryKey::from_mnemonic(words)?;
        let recovered_master_key = config
            .verify_recovery_key(&recovery_key)?
            .ok_or_else(|| Error::Vault("Migrated recovery key failed verification".to_string()))?;
        if recovered_master_key.as_bytes() != original_master_key.as_bytes() {
            return Err(Error::Vault(
                "Recovery key unwraps a different master key".to_string(),
            ));
        }

        provider.upload(&temp_path, config.to_bytes()?).await?;
        if let Err(error) = provider.rename(&config_path, &backup_path).await {
            let _ = provider.delete(&temp_path).await;
            return Err(error);
        }
        if let Err(error) = provider.rename(&temp_path, &config_path).await {
            let restore = provider.rename(&backup_path, &config_path).await;
            return match restore {
                Ok(_) => Err(error),
                Err(restore_error) => Err(Error::Storage(format!(
                    "migration commit failed ({error}); rollback failed ({restore_error})"
                ))),
            };
        }

        let committed = match provider.download(&config_path).await {
            Ok(bytes) => VaultConfig::from_bytes(&bytes),
            Err(error) => Err(error),
        };
        if let Err(verify_error) = committed.and_then(|saved| {
            validate_v1_1_invariants(&saved)?;
            saved.verify_password(password)?.ok_or_else(|| {
                Error::Vault("Committed config failed authentication".to_string())
            })?;
            Ok(())
        }) {
            let _ = provider.delete(&config_path).await;
            if let Err(restore_error) = provider.rename(&backup_path, &config_path).await {
                return Err(Error::Storage(format!(
                    "committed config verification failed ({verify_error}); rollback failed ({restore_error})"
                )));
            }
            return Err(verify_error);
        }

        if let Err(error) = provider.delete(&backup_path).await {
            warn!("Migration committed but backup cleanup failed: {}", error);
        }
        info!("Migration completed successfully to version {}", target);
        Ok(MigrationOutcome {
            config,
            recovery_words,
        })
    }

    async fn recover_interrupted(
        &self,
        provider: &dyn StorageProvider,
        config_path: &VaultPath,
        backup_path: &VaultPath,
        temp_path: &VaultPath,
    ) -> Result<()> {
        let backup_exists = provider.exists(backup_path).await?;
        let config_exists = provider.exists(config_path).await?;
        if backup_exists && !config_exists {
            provider.rename(backup_path, config_path).await?;
        } else if backup_exists {
            return Err(Error::Vault(
                "Unresolved migration backup exists; refusing to overwrite it".to_string(),
            ));
        }
        if provider.exists(temp_path).await? {
            provider.delete(temp_path).await?;
        }
        Ok(())
    }
}

impl Default for MigrationRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// Check whether a vault config requires migration.
pub fn check_migration_needed(config: &VaultConfig) -> MigrationStatus {
    let current = VaultVersion::CURRENT;

    if config.version == current {
        if validate_v1_1_invariants(config).is_err() {
            return MigrationStatus::Incompatible {
                version: config.version,
            };
        }
        return MigrationStatus::UpToDate;
    }

    // Different major version means incompatible.
    if config.version.major != current.major {
        return MigrationStatus::Incompatible {
            version: config.version,
        };
    }

    // Same major, different minor: needs migration if vault is older.
    if config.version.minor < current.minor {
        return MigrationStatus::NeedsMigration {
            from: config.version,
            to: current,
        };
    }

    // Vault is newer than current software.
    MigrationStatus::Incompatible {
        version: config.version,
    }
}

// ---------------------------------------------------------------------------
// Built-in migrations
// ---------------------------------------------------------------------------

fn validate_v1_1_invariants(config: &VaultConfig) -> Result<()> {
    if config.version != VaultVersion::CURRENT
        || config.wrapped_master_key.is_none()
        || config.recovery_wrapped_master_key.is_none()
        || config.recovery_key_verification.is_none()
        || config.encrypted_recovery_key.is_none()
    {
        return Err(Error::Vault(
            "vault config does not satisfy v1.1 key-wrapping and recovery invariants".to_string(),
        ));
    }
    Ok(())
}

/// Authenticated migration from the legacy password-derived-key format to
/// independently wrapped password and recovery keys.
#[allow(non_camel_case_types)]
struct MigrationV1_0ToV1_1;

impl Migration for MigrationV1_0ToV1_1 {
    fn source_version(&self) -> VaultVersion {
        VaultVersion { major: 1, minor: 0 }
    }

    fn target_version(&self) -> VaultVersion {
        VaultVersion { major: 1, minor: 1 }
    }

    fn description(&self) -> &str {
        "Wrap the legacy master key and configure authenticated recovery"
    }

    fn migrate(
        &self,
        config: &mut VaultConfig,
        password: &[u8],
    ) -> Result<Option<Zeroizing<String>>> {
        if config.version != self.source_version() || !config.is_legacy_format() {
            return Err(Error::Vault(
                "v1.0 migration requires an authenticated legacy config".to_string(),
            ));
        }
        config.migrate_to_v1_1(password).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axiomvault_common::VaultId;
    use axiomvault_crypto::KdfParams;

    fn make_test_config(version: VaultVersion) -> VaultConfig {
        let id = VaultId::new("test-vault").unwrap();
        let password = b"test";
        let params = KdfParams::moderate();
        let creation =
            VaultConfig::new(id, password, "local", serde_json::Value::Null, params).unwrap();
        let mut config = creation.config;
        config.version = version;
        config
    }

    #[test]
    fn test_check_migration_up_to_date() {
        let config = make_test_config(VaultVersion::CURRENT);
        assert_eq!(check_migration_needed(&config), MigrationStatus::UpToDate);
    }

    #[test]
    fn test_check_migration_incompatible_major() {
        let config = make_test_config(VaultVersion { major: 2, minor: 0 });
        let status = check_migration_needed(&config);
        assert_eq!(
            status,
            MigrationStatus::Incompatible {
                version: VaultVersion { major: 2, minor: 0 }
            }
        );
    }

    #[test]
    fn test_registry_find_path_same_version() {
        let registry = MigrationRegistry::with_defaults();
        let v1_0 = VaultVersion { major: 1, minor: 0 };
        let path = registry.find_path(&v1_0, &v1_0);
        assert!(path.is_some());
        assert!(path.unwrap().is_empty());
    }

    #[test]
    fn test_registry_find_path_v1_0_to_v1_1() {
        let registry = MigrationRegistry::with_defaults();
        let v1_0 = VaultVersion { major: 1, minor: 0 };
        let v1_1 = VaultVersion { major: 1, minor: 1 };
        let path = registry.find_path(&v1_0, &v1_1);
        assert!(path.is_some());
        let steps = path.unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].source_version(), v1_0);
        assert_eq!(steps[0].target_version(), v1_1);
    }

    #[test]
    fn test_registry_find_path_cross_major_fails() {
        let registry = MigrationRegistry::with_defaults();
        let v1_0 = VaultVersion { major: 1, minor: 0 };
        let v2_0 = VaultVersion { major: 2, minor: 0 };
        assert!(registry.find_path(&v1_0, &v2_0).is_none());
    }

    #[tokio::test]
    async fn test_migration_v1_0_to_v1_1() {
        let password = b"test";
        let mut legacy = make_test_config(VaultVersion { major: 1, minor: 0 });
        legacy.wrapped_master_key = None;
        legacy.recovery_wrapped_master_key = None;
        legacy.recovery_key_verification = None;
        legacy.encrypted_recovery_key = None;
        let original_master_key = legacy.verify_password(password).unwrap().unwrap();
        let provider = axiomvault_storage::MemoryProvider::new();
        provider
            .upload(
                &axiomvault_common::VaultPath::parse(CONFIG_FILENAME).unwrap(),
                legacy.to_bytes().unwrap(),
            )
            .await
            .unwrap();
        let registry = MigrationRegistry::with_defaults();
        let target = VaultVersion { major: 1, minor: 1 };

        let outcome = registry
            .migrate(&provider, password, &target)
            .await
            .unwrap();

        assert_eq!(outcome.config.version, target);
        assert!(outcome.config.wrapped_master_key.is_some());
        assert!(outcome.config.recovery_wrapped_master_key.is_some());
        let recovery_words = outcome
            .recovery_words
            .expect("migration must return recovery words");
        let reopened_master_key = outcome
            .config
            .verify_password(password)
            .unwrap()
            .expect("password must reopen migrated vault");
        assert_eq!(
            reopened_master_key.as_bytes(),
            original_master_key.as_bytes()
        );
        let recovery_key =
            axiomvault_crypto::recovery::RecoveryKey::from_mnemonic(&recovery_words).unwrap();
        let recovered_master_key = outcome
            .config
            .verify_recovery_key(&recovery_key)
            .unwrap()
            .expect("recovery words must reopen migrated vault");
        assert_eq!(
            recovered_master_key.as_bytes(),
            original_master_key.as_bytes()
        );
    }

    #[tokio::test]
    async fn test_migration_already_at_target() {
        let config = make_test_config(VaultVersion { major: 1, minor: 1 });
        let provider = axiomvault_storage::MemoryProvider::new();
        provider
            .upload(
                &VaultPath::parse(CONFIG_FILENAME).unwrap(),
                config.to_bytes().unwrap(),
            )
            .await
            .unwrap();
        let registry = MigrationRegistry::with_defaults();
        let target = VaultVersion { major: 1, minor: 1 };

        let outcome = registry.migrate(&provider, b"test", &target).await.unwrap();
        assert_eq!(outcome.config.version, target);
        assert!(outcome.recovery_words.is_none());
    }

    #[tokio::test]
    async fn test_migration_no_path_fails() {
        let config = make_test_config(VaultVersion { major: 1, minor: 0 });
        let provider = axiomvault_storage::MemoryProvider::new();
        provider
            .upload(
                &VaultPath::parse(CONFIG_FILENAME).unwrap(),
                config.to_bytes().unwrap(),
            )
            .await
            .unwrap();
        let registry = MigrationRegistry::with_defaults();
        let target = VaultVersion { major: 1, minor: 5 };

        let result = registry.migrate(&provider, b"test", &target).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn interrupted_commit_restores_backup_before_migrating() {
        let mut config = make_test_config(VaultVersion { major: 1, minor: 0 });
        config.wrapped_master_key = None;
        config.recovery_wrapped_master_key = None;
        config.recovery_key_verification = None;
        config.encrypted_recovery_key = None;
        let provider = axiomvault_storage::MemoryProvider::new();
        provider
            .upload(
                &VaultPath::parse(CONFIG_BACKUP_FILENAME).unwrap(),
                config.to_bytes().unwrap(),
            )
            .await
            .unwrap();
        let registry = MigrationRegistry::with_defaults();

        let outcome = registry
            .migrate(&provider, b"test", &VaultVersion::CURRENT)
            .await
            .unwrap();
        assert_eq!(outcome.config.version, VaultVersion::CURRENT);
        assert!(provider
            .exists(&VaultPath::parse(CONFIG_FILENAME).unwrap())
            .await
            .unwrap());
        assert!(!provider
            .exists(&VaultPath::parse(CONFIG_BACKUP_FILENAME).unwrap())
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn wrong_password_does_not_modify_legacy_config() {
        let mut legacy = make_test_config(VaultVersion { major: 1, minor: 0 });
        legacy.wrapped_master_key = None;
        legacy.recovery_wrapped_master_key = None;
        legacy.recovery_key_verification = None;
        legacy.encrypted_recovery_key = None;
        let original = legacy.to_bytes().unwrap();
        let provider = axiomvault_storage::MemoryProvider::new();
        provider
            .upload(
                &VaultPath::parse(CONFIG_FILENAME).unwrap(),
                original.clone(),
            )
            .await
            .unwrap();

        let result = MigrationRegistry::with_defaults()
            .migrate(&provider, b"wrong password", &VaultVersion::CURRENT)
            .await;

        assert!(matches!(result, Err(Error::NotPermitted(_))));
        assert_eq!(
            provider
                .download(&VaultPath::parse(CONFIG_FILENAME).unwrap())
                .await
                .unwrap(),
            original
        );
        assert!(!provider
            .exists(&VaultPath::parse(CONFIG_BACKUP_FILENAME).unwrap())
            .await
            .unwrap());
        assert!(!provider
            .exists(&VaultPath::parse(CONFIG_MIGRATION_FILENAME).unwrap())
            .await
            .unwrap());
    }

    #[test]
    fn test_migration_status_display() {
        assert_eq!(MigrationStatus::UpToDate.to_string(), "Up to date");

        let status = MigrationStatus::NeedsMigration {
            from: VaultVersion { major: 1, minor: 0 },
            to: VaultVersion { major: 1, minor: 1 },
        };
        assert_eq!(status.to_string(), "Needs migration from 1.0 to 1.1");

        let status = MigrationStatus::Incompatible {
            version: VaultVersion { major: 2, minor: 0 },
        };
        assert_eq!(status.to_string(), "Incompatible version: 2.0");
    }
}
