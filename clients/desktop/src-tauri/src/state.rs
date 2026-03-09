//! Application state management.

use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::RwLock;

use axiomvault_app::AppService;
use axiomvault_fuse::MountHandle;

/// Per-vault FUSE mount state (presentation concern, not in shared core).
pub struct MountState {
    pub mount_handle: MountHandle,
}

// SAFETY: MountHandle pointers are managed safely by the FUSE library,
// which is thread-safe.
unsafe impl Send for MountState {}
unsafe impl Sync for MountState {}

/// Global application state.
pub struct AppState {
    /// Shared application service (vault lifecycle, file ops, events).
    pub service: AppService,
    /// FUSE mount handles per vault ID (presentation-layer concern).
    pub mounts: RwLock<HashMap<String, MountState>>,
    /// Application data directory.
    pub data_dir: PathBuf,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            service: AppService::new(),
            mounts: RwLock::new(HashMap::new()),
            data_dir,
        }
    }
}
