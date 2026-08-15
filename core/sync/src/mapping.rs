//! Mapping between local persistence paths and remote provider paths.

use axiomvault_common::{Error, Result, VaultPath};

/// Maps canonical local sync paths to provider-specific remote paths.
///
/// Sync state is always keyed by the local path. Implementations must be
/// bijective within the configured roots so state cannot be advanced for a
/// different object than the one persisted locally.
pub trait SyncPathMapper: Send + Sync {
    /// Root scanned on the remote provider.
    fn remote_root(&self) -> VaultPath;
    /// Root used by the local persistence provider.
    fn local_root(&self) -> VaultPath;
    /// Convert a local state path to the corresponding remote provider path.
    fn local_to_remote(&self, local: &VaultPath) -> Result<VaultPath>;
    /// Convert a remote provider path to the canonical local state path.
    fn remote_to_local(&self, remote: &VaultPath) -> Result<VaultPath>;
}

/// Identity mapping for providers that use the same vault paths.
#[derive(Debug, Default)]
pub struct IdentityPathMapper;

impl SyncPathMapper for IdentityPathMapper {
    fn remote_root(&self) -> VaultPath {
        VaultPath::root()
    }

    fn local_root(&self) -> VaultPath {
        VaultPath::root()
    }

    fn local_to_remote(&self, local: &VaultPath) -> Result<VaultPath> {
        Ok(local.clone())
    }

    fn remote_to_local(&self, remote: &VaultPath) -> Result<VaultPath> {
        Ok(remote.clone())
    }
}

/// Maps paths below one local prefix to paths below a different remote prefix.
#[derive(Debug, Clone)]
pub struct PrefixPathMapper {
    local_prefix: VaultPath,
    remote_prefix: VaultPath,
}

impl PrefixPathMapper {
    /// Create a prefix mapper.
    pub fn new(local_prefix: VaultPath, remote_prefix: VaultPath) -> Self {
        Self {
            local_prefix,
            remote_prefix,
        }
    }

    fn remap(path: &VaultPath, from: &VaultPath, to: &VaultPath) -> Result<VaultPath> {
        let suffix = path
            .components()
            .strip_prefix(from.components())
            .ok_or_else(|| {
                Error::InvalidInput(format!("path {path} is outside mapped prefix {from}"))
            })?;
        let mut mapped = to.clone();
        for component in suffix {
            mapped = mapped.join(component)?;
        }
        Ok(mapped)
    }
}

impl SyncPathMapper for PrefixPathMapper {
    fn remote_root(&self) -> VaultPath {
        self.remote_prefix.clone()
    }

    fn local_root(&self) -> VaultPath {
        self.local_prefix.clone()
    }

    fn local_to_remote(&self, local: &VaultPath) -> Result<VaultPath> {
        Self::remap(local, &self.local_prefix, &self.remote_prefix)
    }

    fn remote_to_local(&self, remote: &VaultPath) -> Result<VaultPath> {
        Self::remap(remote, &self.remote_prefix, &self.local_prefix)
    }
}
