//! Vault format migration framework.
//!
//! Provides versioned migrations for upgrading vault formats between versions.
//! Migrations are executed in sequence with automatic backup and rollback support.

use std::fmt;
use std::path::Path;

use tracing::{info, warn};

use crate::config::{VaultConfig, VaultVersion, CONFIG_FILENAME};
use axiomvault_common::{Error, Result};

/// Backup filename for vault config during migration.
pub const CONFIG_BACKUP_FILENAME: &str = "vault.config.backup";

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
    /// Execute the migration, modifying the vault config and optionally vault files.
    fn migrate(&self, vault_path: &Path, config: &mut VaultConfig) -> Result<()>;
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
            let next = self.migrations.iter().find(|m| m.source_version() == current);
            match next {
                Some(migration) => {
                    let target = migration.target_version();
                    // Ensure we're moving forward and not past our target.
                    if target.minor > to.minor || target.major != to.major {
                        return None;
                    }
                    path.push(migration.as_ref());
                    current = target;
                }
                None => return None,
            }
        }

        Some(path)
    }

    /// Execute all migrations from the vault's current version to the target version.
    ///
    /// Backs up the vault config before starting. If any step fails, the backup
    /// is restored and the error is returned.
    pub fn migrate(
        &self,
        vault_path: &Path,
        config: &mut VaultConfig,
        target: &VaultVersion,
    ) -> Result<()> {
        let from = config.version;
        let path = self.find_path(&from, target).ok_or_else(|| {
            Error::Vault(format!("No migration path from {} to {}", from, target))
        })?;

        if path.is_empty() {
            info!("Vault is already at version {}", target);
            return Ok(());
        }

        info!(
            "Migrating vault from {} to {} ({} step(s))",
            from,
            target,
            path.len()
        );

        // Backup config before migration.
        self.backup_config(vault_path)?;

        for (i, migration) in path.iter().enumerate() {
            info!(
                "  Step {}/{}: {} -> {} - {}",
                i + 1,
                path.len(),
                migration.source_version(),
                migration.target_version(),
                migration.description()
            );

            if let Err(e) = migration.migrate(vault_path, config) {
                warn!("Migration step failed: {}. Restoring backup.", e);
                if let Err(restore_err) = self.restore_config(vault_path, config) {
                    warn!("Failed to restore backup: {}", restore_err);
                }
                return Err(e);
            }
        }

        // Verify the final version matches the target.
        if config.version != *target {
            warn!(
                "Version mismatch after migration: expected {}, got {}",
                target, config.version
            );
            if let Err(restore_err) = self.restore_config(vault_path, config) {
                warn!("Failed to restore backup: {}", restore_err);
            }
            return Err(Error::Vault(format!(
                "Migration completed but version is {} instead of {}",
                config.version, target
            )));
        }

        // Save the updated config.
        self.save_config(vault_path, config)?;

        // Remove backup after successful migration.
        let backup_path = vault_path.join(CONFIG_BACKUP_FILENAME);
        if backup_path.exists() {
            let _ = std::fs::remove_file(&backup_path);
        }

        info!("Migration completed successfully to version {}", target);
        Ok(())
    }

    /// Backup the vault config file.
    fn backup_config(&self, vault_path: &Path) -> Result<()> {
        let config_path = vault_path.join(CONFIG_FILENAME);
        let backup_path = vault_path.join(CONFIG_BACKUP_FILENAME);

        if config_path.exists() {
            std::fs::copy(&config_path, &backup_path)
                .map_err(|e| Error::Vault(format!("Failed to backup config: {}", e)))?;
            info!("Config backed up to {}", backup_path.display());
        }

        Ok(())
    }

    /// Restore vault config from backup.
    fn restore_config(&self, vault_path: &Path, config: &mut VaultConfig) -> Result<()> {
        let backup_path = vault_path.join(CONFIG_BACKUP_FILENAME);

        if backup_path.exists() {
            let backup_bytes = std::fs::read(&backup_path)
                .map_err(|e| Error::Vault(format!("Failed to read backup: {}", e)))?;
            let backup_config = VaultConfig::from_bytes(&backup_bytes)?;
            *config = backup_config;

            // Restore the config file as well.
            let config_path = vault_path.join(CONFIG_FILENAME);
            std::fs::copy(&backup_path, &config_path)
                .map_err(|e| Error::Vault(format!("Failed to restore config file: {}", e)))?;

            info!("Config restored from backup");
        }

        Ok(())
    }

    /// Save config to the vault path.
    fn save_config(&self, vault_path: &Path, config: &VaultConfig) -> Result<()> {
        let config_path = vault_path.join(CONFIG_FILENAME);
        let config_bytes = config.to_bytes()?;
        std::fs::write(&config_path, config_bytes)
            .map_err(|e| Error::Vault(format!("Failed to save config: {}", e)))?;
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

/// Placeholder migration from v1.0 to v1.1.
///
/// This migration does not change vault structure; it simply bumps the version
/// to validate the migration framework end-to-end.
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
        "Bump vault format version (no structural changes)"
    }

    fn migrate(&self, _vault_path: &Path, config: &mut VaultConfig) -> Result<()> {
        config.version = self.target_version();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axiomvault_common::VaultId;
    use axiomvault_crypto::KdfParams;
    use tempfile::TempDir;

    fn make_test_config(version: VaultVersion) -> VaultConfig {
        let id = VaultId::new("test-vault").unwrap();
        let password = b"test";
        let params = KdfParams::moderate();
        let mut config =
            VaultConfig::new(id, password, "local", serde_json::Value::Null, params).unwrap();
        config.version = version;
        config
    }

    fn setup_vault_dir(config: &VaultConfig) -> TempDir {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join(CONFIG_FILENAME);
        let config_bytes = config.to_bytes().unwrap();
        std::fs::write(&config_path, config_bytes).unwrap();
        dir
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

    #[test]
    fn test_migration_v1_0_to_v1_1() {
        let mut config = make_test_config(VaultVersion { major: 1, minor: 0 });
        let dir = setup_vault_dir(&config);
        let registry = MigrationRegistry::with_defaults();
        let target = VaultVersion { major: 1, minor: 1 };

        registry.migrate(dir.path(), &mut config, &target).unwrap();

        assert_eq!(config.version, target);

        // Verify config was persisted.
        let config_path = dir.path().join(CONFIG_FILENAME);
        let saved_bytes = std::fs::read(&config_path).unwrap();
        let saved_config = VaultConfig::from_bytes(&saved_bytes).unwrap();
        assert_eq!(saved_config.version, target);

        // Backup should have been cleaned up.
        let backup_path = dir.path().join(CONFIG_BACKUP_FILENAME);
        assert!(!backup_path.exists());
    }

    #[test]
    fn test_migration_already_at_target() {
        let mut config = make_test_config(VaultVersion { major: 1, minor: 1 });
        let dir = setup_vault_dir(&config);
        let registry = MigrationRegistry::with_defaults();
        let target = VaultVersion { major: 1, minor: 1 };

        registry.migrate(dir.path(), &mut config, &target).unwrap();
        assert_eq!(config.version, target);
    }

    #[test]
    fn test_migration_no_path_fails() {
        let mut config = make_test_config(VaultVersion { major: 1, minor: 0 });
        let dir = setup_vault_dir(&config);
        let registry = MigrationRegistry::with_defaults();
        // No migration registered for 1.0 -> 1.5
        let target = VaultVersion { major: 1, minor: 5 };

        let result = registry.migrate(dir.path(), &mut config, &target);
        assert!(result.is_err());
    }

    #[test]
    fn test_backup_and_restore() {
        let config = make_test_config(VaultVersion { major: 1, minor: 0 });
        let dir = setup_vault_dir(&config);
        let registry = MigrationRegistry::with_defaults();

        // Backup.
        registry.backup_config(dir.path()).unwrap();
        let backup_path = dir.path().join(CONFIG_BACKUP_FILENAME);
        assert!(backup_path.exists());

        // Restore.
        let mut restored_config = make_test_config(VaultVersion {
            major: 99,
            minor: 0,
        });
        registry
            .restore_config(dir.path(), &mut restored_config)
            .unwrap();
        assert_eq!(restored_config.version, config.version);
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
