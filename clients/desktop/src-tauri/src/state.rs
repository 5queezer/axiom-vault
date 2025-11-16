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
    pub config_path: PathBuf,
    pub mount_handle: Option<MountHandle>,
}

/// Global application state.
pub struct AppState {
    /// Map of vault ID to open vault.
    pub vaults: RwLock<HashMap<String, OpenVault>>,
    /// Application data directory.
    pub data_dir: PathBuf,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            vaults: RwLock::new(HashMap::new()),
            data_dir,
        }
    }
}
