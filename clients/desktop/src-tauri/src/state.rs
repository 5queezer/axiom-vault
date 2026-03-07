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

// SAFETY: All fields except mount_handle are Arc/owned. MountHandle pointers
// are managed safely by the FUSE library, which is thread-safe.
unsafe impl Send for OpenVault {}

// SAFETY: All mutable access is through Arc/RwLock, making it safe for concurrent access.
unsafe impl Sync for OpenVault {}

/// Global application state.
pub struct AppState {
    /// Map of vault ID to open vault.
    pub vaults: RwLock<HashMap<String, OpenVault>>,
    /// Application data directory.
    pub data_dir: PathBuf,
}

// SAFETY: All fields are arc/rwlock-wrapped or tokio types that are Send+Sync.
// MountHandle pointers are managed safely by the FUSE library, and all
// access to state is guarded by RwLock, ensuring thread-safe access.
unsafe impl Send for AppState {}
// SAFETY: All fields are arc/rwlock-wrapped or tokio types that are Sync.
unsafe impl Sync for AppState {}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            vaults: RwLock::new(HashMap::new()),
            data_dir,
        }
    }
}
