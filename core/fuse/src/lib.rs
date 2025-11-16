//! FUSE filesystem adapter for AxiomVault.
//!
//! This module provides FUSE mount support for Linux and macOS,
//! allowing vaults to be mounted as virtual filesystems.
//!
//! # Architecture
//! The FUSE adapter translates filesystem operations into vault operations,
//! handling all encryption/decryption transparently through the vault engine.
//!
//! # Feature Flags
//! - `fuse`: Enable FUSE support (requires libfuse3-dev on Linux or macFUSE on macOS)

#[cfg(feature = "fuse")]
pub mod filesystem;

#[cfg(feature = "fuse")]
pub mod mount;

#[cfg(feature = "fuse")]
pub use filesystem::VaultFilesystem;

#[cfg(feature = "fuse")]
pub use mount::{MountHandle, MountOptions};

/// Stub module for when FUSE is not available.
#[cfg(not(feature = "fuse"))]
pub mod mount {
    use axiomvault_common::{Error, Result};
    use std::path::{Path, PathBuf};

    /// Mount options placeholder.
    #[derive(Debug, Clone, Default)]
    pub struct MountOptions {
        pub allow_other: bool,
        pub auto_unmount: bool,
        pub read_only: bool,
        pub default_permissions: bool,
    }

    /// Mount handle placeholder.
    pub struct MountHandle {
        mount_point: PathBuf,
    }

    impl MountHandle {
        pub fn mount_point(&self) -> &Path {
            &self.mount_point
        }

        pub fn unmount(self) {
            drop(self);
        }
    }

    /// Check if FUSE is available (always false without feature).
    pub fn is_fuse_available() -> bool {
        false
    }

    /// Get FUSE info message.
    pub fn fuse_info() -> String {
        "FUSE support not compiled in. Rebuild with --features fuse".to_string()
    }

    /// Mount stub - always returns error.
    pub fn mount<P: AsRef<Path>>(
        _session: std::sync::Arc<axiomvault_vault::VaultSession>,
        _mount_point: P,
        _options: MountOptions,
        _runtime: tokio::runtime::Handle,
    ) -> Result<MountHandle> {
        Err(Error::NotPermitted(
            "FUSE support not compiled. Rebuild with --features fuse".to_string(),
        ))
    }
}

#[cfg(not(feature = "fuse"))]
pub use mount::{fuse_info, is_fuse_available, MountHandle, MountOptions};
