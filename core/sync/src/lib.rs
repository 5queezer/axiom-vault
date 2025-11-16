//! AxiomVault Sync Engine
//!
//! This module provides synchronization capabilities for AxiomVault, including:
//! - Two sync modes: on-demand and periodic
//! - Local staging area for atomic writes
//! - Conflict detection and resolution
//! - Retry strategy with exponential backoff
//! - Background task coordination

pub mod conflict;
pub mod engine;
pub mod retry;
pub mod scheduler;
pub mod staging;
pub mod state;

// Re-export main types
pub use conflict::{ConflictInfo, ConflictResolver, ConflictStrategy, ResolutionResult};
pub use engine::{SyncConfig, SyncEngine};
pub use retry::{RetryConfig, RetryExecutor, retry, retry_with_config};
pub use scheduler::{SyncMode, SyncRequest, SyncResult, SyncScheduler, SyncSchedulerHandle};
pub use staging::{ChangeType, StagedChange, StagingArea};
pub use state::{SyncEntry, SyncState, SyncStatus};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        // Verify all main types are accessible
        let _config = SyncConfig::default();
        let _retry_config = RetryConfig::default();
        let _resolver = ConflictResolver::default();
        let _state = SyncState::new();
    }
}
