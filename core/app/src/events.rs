//! Event system for cross-layer state synchronization.
//!
//! UI shells subscribe to [`AppEvent`] via a tokio broadcast channel.
//! Events are fire-and-forget from the core's perspective — a slow or
//! disconnected receiver does not block the sender.

use serde::{Deserialize, Serialize};

use crate::dto::{DirectoryEntryDto, VaultInfoDto};

/// Broadcast channel sender.
pub type EventSender = tokio::sync::broadcast::Sender<AppEvent>;

/// Broadcast channel receiver.
pub type EventReceiver = tokio::sync::broadcast::Receiver<AppEvent>;

/// Create a new event channel with the given capacity.
pub fn event_channel(capacity: usize) -> (EventSender, EventReceiver) {
    tokio::sync::broadcast::channel(capacity)
}

/// Application events broadcast to UI shells.
///
/// Each variant carries enough data for the UI to update without
/// needing to call back into `AppService`. Serializable so that
/// FFI/bridge layers can marshal events as JSON without ad-hoc conversion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AppEvent {
    // -- Vault lifecycle --
    /// A vault was created and is now open.
    VaultCreated(VaultInfoDto),

    /// A vault was opened (unlocked).
    VaultOpened(VaultInfoDto),

    /// The active vault was locked.
    VaultLocked,

    /// The active vault was closed.
    VaultClosed,

    /// Vault password was changed.
    PasswordChanged,

    // -- File operations --
    /// A file was created at the given path.
    FileCreated { path: String },

    /// A file was updated at the given path.
    FileUpdated { path: String },

    /// A file was deleted at the given path.
    FileDeleted { path: String },

    // -- Directory operations --
    /// A directory was created.
    DirectoryCreated { path: String },

    /// A directory was deleted.
    DirectoryDeleted { path: String },

    /// Directory listing was refreshed.
    DirectoryListed {
        path: String,
        entries: Vec<DirectoryEntryDto>,
    },

    // -- Sync --
    /// Sync started.
    SyncStarted,

    /// Sync completed successfully.
    SyncCompleted,

    /// Sync failed.
    SyncFailed { error: String },

    // -- Errors --
    /// A non-fatal error occurred.
    Error { message: String },
}
