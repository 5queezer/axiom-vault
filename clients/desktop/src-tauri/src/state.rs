//! Application state management.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use axiomvault_fuse::MountHandle;
use axiomvault_vault::VaultSession;

use crate::local_index::LocalIndex;

/// Information about an open vault.
pub struct OpenVault {
    pub session: Arc<VaultSession>,
    pub index: Arc<LocalIndex>,
    #[allow(dead_code)]
    pub config_path: PathBuf,
    pub mount_handle: Option<MountHandle>,
}

// SAFETY: OpenVault is safe to share across threads.
// MountHandle pointers are managed safely by the FUSE library.
unsafe impl Send for OpenVault {}
unsafe impl Sync for OpenVault {}

/// Global application state.
pub struct AppState {
    /// Map of vault ID to open vault.
    pub vaults: RwLock<HashMap<String, OpenVault>>,
    /// Application data directory.
    pub data_dir: PathBuf,
}

// SAFETY: AppState is safe to share across threads.
// The MountHandle contains FUSE pointers which are managed safely by the FUSE library.
// All access to vaults is guarded by RwLock which ensures thread-safe access.
unsafe impl Send for AppState {}
unsafe impl Sync for AppState {}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            vaults: RwLock::new(HashMap::new()),
            data_dir,
        }
    }
}
