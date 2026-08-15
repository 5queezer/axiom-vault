//! Trusted local monotonic freshness anchors.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use axiomvault_common::{Error, Result, VaultId};

/// Trusted state kept outside the storage provider controlled by an attacker.
pub trait FreshnessAnchor: Send + Sync {
    /// Return the latest locally accepted generation, if this device has one.
    fn load(&self, vault_id: &VaultId) -> Result<Option<u64>>;

    /// Advance the anchor. Implementations must reject decreasing generations.
    fn store(&self, vault_id: &VaultId, generation: u64) -> Result<()>;
}

/// Anchor used when the platform cannot provide trusted local storage.
/// Every operation fails closed rather than silently disabling protection.
pub struct UnavailableFreshnessAnchor {
    reason: String,
}

impl UnavailableFreshnessAnchor {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl FreshnessAnchor for UnavailableFreshnessAnchor {
    fn load(&self, _vault_id: &VaultId) -> Result<Option<u64>> {
        Err(Error::Vault(format!(
            "freshness anchor unavailable: {}",
            self.reason
        )))
    }

    fn store(&self, _vault_id: &VaultId, _generation: u64) -> Result<()> {
        Err(Error::Vault(format!(
            "freshness anchor unavailable: {}",
            self.reason
        )))
    }
}

/// Process-local anchor useful for tests and explicitly ephemeral clients.
#[derive(Default)]
pub struct InMemoryFreshnessAnchor {
    generations: RwLock<HashMap<String, u64>>,
}

impl InMemoryFreshnessAnchor {
    pub fn new() -> Self {
        Self::default()
    }
}

impl FreshnessAnchor for InMemoryFreshnessAnchor {
    fn load(&self, vault_id: &VaultId) -> Result<Option<u64>> {
        let generations = self
            .generations
            .read()
            .map_err(|_| Error::Vault("freshness anchor lock poisoned".to_string()))?;
        Ok(generations.get(vault_id.as_str()).copied())
    }

    fn store(&self, vault_id: &VaultId, generation: u64) -> Result<()> {
        let mut generations = self
            .generations
            .write()
            .map_err(|_| Error::Vault("freshness anchor lock poisoned".to_string()))?;
        if generations
            .get(vault_id.as_str())
            .is_some_and(|current| generation < *current)
        {
            return Err(Error::Conflict(
                "refusing to decrease freshness anchor".to_string(),
            ));
        }
        generations.insert(vault_id.as_str().to_string(), generation);
        Ok(())
    }
}

/// File-backed anchor stored outside the vault's storage provider.
pub struct LocalFileFreshnessAnchor {
    directory: PathBuf,
    lock: RwLock<()>,
}

impl LocalFileFreshnessAnchor {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            lock: RwLock::new(()),
        }
    }

    pub fn platform_default() -> Result<Self> {
        let base = dirs::data_local_dir().ok_or_else(|| {
            Error::Vault("local data directory unavailable for freshness anchor".to_string())
        })?;
        Ok(Self::new(base.join("axiomvault").join("freshness")))
    }

    fn path(&self, vault_id: &VaultId) -> PathBuf {
        self.directory
            .join(format!("{}.generation", vault_id.as_str()))
    }

    fn read_generation(path: &Path) -> Result<Option<u64>> {
        match fs::read_to_string(path) {
            Ok(value) => value.trim().parse::<u64>().map(Some).map_err(|_| {
                Error::Vault("freshness anchor is corrupt; refusing to open vault".to_string())
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(Error::Vault(format!(
                "failed to read freshness anchor: {error}"
            ))),
        }
    }
}

impl FreshnessAnchor for LocalFileFreshnessAnchor {
    fn load(&self, vault_id: &VaultId) -> Result<Option<u64>> {
        let _guard = self
            .lock
            .read()
            .map_err(|_| Error::Vault("freshness anchor lock poisoned".to_string()))?;
        Self::read_generation(&self.path(vault_id))
    }

    fn store(&self, vault_id: &VaultId, generation: u64) -> Result<()> {
        let _guard = self
            .lock
            .write()
            .map_err(|_| Error::Vault("freshness anchor lock poisoned".to_string()))?;
        fs::create_dir_all(&self.directory).map_err(|error| {
            Error::Vault(format!(
                "failed to create freshness anchor directory: {error}"
            ))
        })?;
        let path = self.path(vault_id);
        if Self::read_generation(&path)?.is_some_and(|current| generation < current) {
            return Err(Error::Conflict(
                "refusing to decrease freshness anchor".to_string(),
            ));
        }

        let temporary = path.with_extension("generation.tmp");
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| Error::Vault(format!("failed to write freshness anchor: {error}")))?;
        writeln!(file, "{generation}")
            .map_err(|error| Error::Vault(format!("failed to write freshness anchor: {error}")))?;
        file.sync_all()
            .map_err(|error| Error::Vault(format!("failed to sync freshness anchor: {error}")))?;
        fs::rename(&temporary, &path).map_err(|error| {
            Error::Vault(format!("failed to replace freshness anchor: {error}"))
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_anchor_persists_and_never_decreases() {
        let directory = tempfile::tempdir().unwrap();
        let vault_id = VaultId::new("anchor-test").unwrap();
        let anchor = LocalFileFreshnessAnchor::new(directory.path());

        assert_eq!(anchor.load(&vault_id).unwrap(), None);
        anchor.store(&vault_id, 4).unwrap();
        assert_eq!(anchor.load(&vault_id).unwrap(), Some(4));
        assert!(anchor.store(&vault_id, 3).is_err());
        assert_eq!(anchor.load(&vault_id).unwrap(), Some(4));
    }

    #[test]
    fn corrupt_local_anchor_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let vault_id = VaultId::new("corrupt-anchor").unwrap();
        let anchor = LocalFileFreshnessAnchor::new(directory.path());
        fs::create_dir_all(directory.path()).unwrap();
        fs::write(anchor.path(&vault_id), b"not-a-generation").unwrap();

        assert!(anchor.load(&vault_id).is_err());
    }
}
