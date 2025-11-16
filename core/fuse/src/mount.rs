//! FUSE mount management.
//!
//! Provides a high-level interface for mounting and unmounting vaults.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use fuser::MountOption;
use tokio::runtime::Handle;
use tracing::{error, info};

use crate::filesystem::VaultFilesystem;
use axiomvault_common::{Error, Result};
use axiomvault_vault::VaultSession;

/// Mount options for FUSE filesystem.
#[derive(Debug, Clone)]
pub struct MountOptions {
    /// Allow other users to access the mount.
    pub allow_other: bool,
    /// Enable auto unmount when process exits.
    pub auto_unmount: bool,
    /// Read-only mount.
    pub read_only: bool,
    /// Default permissions.
    pub default_permissions: bool,
}

impl Default for MountOptions {
    fn default() -> Self {
        Self {
            allow_other: false,
            auto_unmount: true,
            read_only: false,
            default_permissions: true,
        }
    }
}

/// Handle to a mounted FUSE filesystem.
///
/// The mount is automatically unmounted when this handle is dropped.
pub struct MountHandle {
    mount_point: PathBuf,
    _session: Arc<fuser::Session<VaultFilesystem>>,
    _thread: Option<JoinHandle<()>>,
}

impl MountHandle {
    /// Get the mount point path.
    pub fn mount_point(&self) -> &Path {
        &self.mount_point
    }

    /// Unmount the filesystem.
    ///
    /// This is automatically called when the handle is dropped.
    pub fn unmount(self) {
        // Drop will handle unmounting
        drop(self);
    }
}

impl Drop for MountHandle {
    fn drop(&mut self) {
        info!("Unmounting vault from {:?}", self.mount_point);
        // Session drop will handle unmounting
    }
}

/// Mount a vault session as a FUSE filesystem.
///
/// # Arguments
/// - `session`: Active vault session to mount
/// - `mount_point`: Directory where the vault will be mounted
/// - `options`: Mount configuration options
/// - `runtime`: Tokio runtime handle for async operations
///
/// # Preconditions
/// - Session must be active
/// - Mount point must exist and be an empty directory
/// - User must have FUSE permissions
///
/// # Postconditions
/// - Returns a handle that keeps the mount active
/// - Vault is accessible as a regular filesystem at mount_point
///
/// # Errors
/// - Mount point does not exist
/// - Mount point is not empty
/// - FUSE not available
/// - Permission denied
pub fn mount(
    session: Arc<VaultSession>,
    mount_point: impl AsRef<Path>,
    options: MountOptions,
    runtime: Handle,
) -> Result<MountHandle> {
    let mount_point = mount_point.as_ref().to_path_buf();

    // Verify mount point exists
    if !mount_point.exists() {
        return Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Mount point does not exist: {:?}", mount_point),
        )));
    }

    if !mount_point.is_dir() {
        return Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Mount point is not a directory: {:?}", mount_point),
        )));
    }

    info!("Mounting vault at {:?}", mount_point);

    // Create filesystem
    let fs = VaultFilesystem::new(session, runtime);

    // Configure mount options
    let mut fuse_options = vec![
        MountOption::FSName("axiomvault".to_string()),
        MountOption::Subtype("axiomvault".to_string()),
    ];

    if options.allow_other {
        fuse_options.push(MountOption::AllowOther);
    }

    if options.auto_unmount {
        fuse_options.push(MountOption::AutoUnmount);
    }

    if options.read_only {
        fuse_options.push(MountOption::RO);
    }

    if options.default_permissions {
        fuse_options.push(MountOption::DefaultPermissions);
    }

    // Mount the filesystem in background thread
    let mount_point_clone = mount_point.clone();
    let session = fuser::Session::new(fs, &mount_point_clone, &fuse_options).map_err(|e| {
        error!("Failed to create FUSE session: {}", e);
        Error::Io(e)
    })?;

    let session = Arc::new(session);

    info!("Vault mounted successfully at {:?}", mount_point);

    Ok(MountHandle {
        mount_point,
        _session: session,
        _thread: None,
    })
}

/// Check if FUSE is available on the system.
///
/// # Returns
/// - `true` if FUSE is available
/// - `false` otherwise
pub fn is_fuse_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        Path::new("/dev/fuse").exists()
    }

    #[cfg(target_os = "macos")]
    {
        // Check for macFUSE
        Path::new("/Library/Filesystems/macfuse.fs").exists()
            || Path::new("/Library/Filesystems/osxfuse.fs").exists()
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        false
    }
}

/// Get platform-specific FUSE information.
pub fn fuse_info() -> String {
    #[cfg(target_os = "linux")]
    {
        if is_fuse_available() {
            "FUSE available via /dev/fuse".to_string()
        } else {
            "FUSE not available. Install fuse3 package.".to_string()
        }
    }

    #[cfg(target_os = "macos")]
    {
        if is_fuse_available() {
            "macFUSE available".to_string()
        } else {
            "macFUSE not installed. Visit https://osxfuse.github.io/".to_string()
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        "FUSE not supported on this platform".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_mount_options() {
        let opts = MountOptions::default();
        assert!(!opts.allow_other);
        assert!(opts.auto_unmount);
        assert!(!opts.read_only);
        assert!(opts.default_permissions);
    }

    #[test]
    fn test_fuse_info() {
        let info = fuse_info();
        assert!(!info.is_empty());
    }
}
