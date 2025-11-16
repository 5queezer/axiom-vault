//! Core sync engine that orchestrates all sync operations.

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn, error};

use axiomvault_common::{Result, Error, VaultPath};
use axiomvault_storage::StorageProvider;

use crate::conflict::{ConflictInfo, ConflictResolver, ConflictStrategy, ResolutionResult};
use crate::retry::{RetryConfig, RetryExecutor};
use crate::scheduler::{SyncMode, SyncRequest, SyncResult, SyncScheduler, SyncSchedulerHandle};
use crate::staging::{ChangeType, StagingArea};
use crate::state::{SyncEntry, SyncState, SyncStatus};

/// Configuration for the sync engine.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SyncConfig {
    /// Maximum number of retries for network operations.
    pub max_retries: u32,
    /// Conflict resolution strategy.
    pub conflict_strategy: ConflictStrategy,
    /// Sync mode.
    pub sync_mode: SyncMode,
    /// Batch size for syncing multiple files.
    pub batch_size: usize,
    /// Whether to automatically resolve conflicts.
    pub auto_resolve_conflicts: bool,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            conflict_strategy: ConflictStrategy::KeepBoth,
            sync_mode: SyncMode::Manual,
            batch_size: 10,
            auto_resolve_conflicts: false,
        }
    }
}

/// Main sync engine for coordinating vault synchronization.
pub struct SyncEngine<P: StorageProvider + ?Sized> {
    /// Storage provider for remote operations.
    provider: Arc<P>,
    /// Sync state tracking.
    state: Arc<RwLock<SyncState>>,
    /// Staging area for atomic writes.
    staging: Arc<RwLock<StagingArea>>,
    /// Conflict resolver.
    conflict_resolver: Arc<ConflictResolver>,
    /// Retry executor.
    retry_executor: Arc<RetryExecutor>,
    /// Sync scheduler.
    scheduler: Option<SyncScheduler>,
    /// Configuration.
    config: SyncConfig,
}

impl<P: StorageProvider + 'static> SyncEngine<P> {
    /// Create a new sync engine.
    pub async fn new(
        provider: P,
        staging_dir: impl AsRef<std::path::Path>,
        config: SyncConfig,
    ) -> Result<Self> {
        let staging = StagingArea::new(staging_dir).await?;
        let retry_config = RetryConfig::new(config.max_retries);
        let conflict_resolver = ConflictResolver::new(config.conflict_strategy);

        Ok(Self {
            provider: Arc::new(provider),
            state: Arc::new(RwLock::new(SyncState::new())),
            staging: Arc::new(RwLock::new(staging)),
            conflict_resolver: Arc::new(conflict_resolver),
            retry_executor: Arc::new(RetryExecutor::new(retry_config)),
            scheduler: None,
            config,
        })
    }
}

impl<P: StorageProvider + ?Sized + 'static> SyncEngine<P> {
    /// Create a new sync engine from an Arc-wrapped provider.
    pub async fn from_arc(
        provider: Arc<P>,
        staging_dir: impl AsRef<std::path::Path>,
        config: SyncConfig,
    ) -> Result<Self> {
        let staging = StagingArea::new(staging_dir).await?;
        let retry_config = RetryConfig::new(config.max_retries);
        let conflict_resolver = ConflictResolver::new(config.conflict_strategy);

        Ok(Self {
            provider,
            state: Arc::new(RwLock::new(SyncState::new())),
            staging: Arc::new(RwLock::new(staging)),
            conflict_resolver: Arc::new(conflict_resolver),
            retry_executor: Arc::new(RetryExecutor::new(retry_config)),
            scheduler: None,
            config,
        })
    }

    /// Initialize the scheduler and return a handle for running it.
    pub fn init_scheduler(&mut self) -> SyncSchedulerHandle {
        let (scheduler, handle) = SyncScheduler::new(self.config.sync_mode.clone());
        self.scheduler = Some(scheduler);
        handle
    }

    /// Get the scheduler for requesting syncs.
    pub fn scheduler(&self) -> Option<&SyncScheduler> {
        self.scheduler.as_ref()
    }

    /// Get a reference to the sync state.
    pub fn state(&self) -> Arc<RwLock<SyncState>> {
        self.state.clone()
    }

    /// Get a reference to the staging area.
    pub fn staging(&self) -> Arc<RwLock<StagingArea>> {
        self.staging.clone()
    }

    /// Stage a local file change for sync.
    pub async fn stage_change(
        &self,
        path: &VaultPath,
        data: Vec<u8>,
        change_type: ChangeType,
    ) -> Result<String> {
        let mut staging = self.staging.write().await;
        let change_id = staging.stage_upload(path, data, change_type).await?;

        // Update sync state
        let mut state = self.state.write().await;
        let etag = Some(uuid::Uuid::new_v4().to_string());

        if let Some(entry) = state.get_mut(path) {
            entry.mark_local_modified(etag);
        } else {
            state.insert(SyncEntry::new_local(path.to_string(), etag));
        }

        Ok(change_id)
    }

    /// Stage a file deletion.
    pub async fn stage_delete(&self, path: &VaultPath) -> Result<String> {
        let mut staging = self.staging.write().await;
        let change_id = staging.stage_delete(path).await?;

        // Update sync state
        let mut state = self.state.write().await;
        if let Some(entry) = state.get_mut(path) {
            entry.mark_local_modified(None);
        } else {
            state.insert(SyncEntry::new_local(path.to_string(), None));
        }

        Ok(change_id)
    }

    /// Perform a full sync of all staged changes and fetch remote updates.
    pub async fn sync_full(&self) -> Result<SyncResult> {
        let start = Instant::now();
        let mut files_synced = 0;
        let mut files_failed = 0;
        let mut conflicts_found = 0;

        info!("Starting full sync");

        {
            let mut state = self.state.write().await;
            state.sync_in_progress = true;
        }

        // 1. Upload local changes
        let upload_result = self.upload_staged_changes().await;
        files_synced += upload_result.0;
        files_failed += upload_result.1;
        conflicts_found += upload_result.2;

        // 2. Check for remote changes
        let remote_result = self.check_remote_changes().await;
        conflicts_found += remote_result.unwrap_or(0);

        // 3. Download remote changes
        let download_result = self.download_remote_changes().await;
        files_synced += download_result.0;
        files_failed += download_result.1;

        {
            let mut state = self.state.write().await;
            state.sync_in_progress = false;
            state.last_full_sync = Some(chrono::Utc::now());
        }

        let duration = start.elapsed();
        info!(
            "Full sync completed in {:?}: {} synced, {} failed, {} conflicts",
            duration, files_synced, files_failed, conflicts_found
        );

        Ok(SyncResult {
            files_synced,
            files_failed,
            conflicts_found,
            duration,
        })
    }

    /// Sync specific paths only.
    pub async fn sync_paths(&self, paths: Vec<String>) -> Result<SyncResult> {
        let start = Instant::now();
        let mut files_synced = 0;
        let mut files_failed = 0;
        let mut conflicts_found = 0;

        info!("Syncing {} specific paths", paths.len());

        for path_str in paths {
            let path = match VaultPath::parse(&path_str) {
                Ok(p) => p,
                Err(e) => {
                    warn!("Invalid path {}: {}", path_str, e);
                    files_failed += 1;
                    continue;
                }
            };

            match self.sync_single_path(&path).await {
                Ok(result) => {
                    if result.has_conflict {
                        conflicts_found += 1;
                    } else {
                        files_synced += 1;
                    }
                }
                Err(e) => {
                    error!("Failed to sync {}: {}", path_str, e);
                    files_failed += 1;
                }
            }
        }

        let duration = start.elapsed();
        Ok(SyncResult {
            files_synced,
            files_failed,
            conflicts_found,
            duration,
        })
    }

    /// Process a sync request (for scheduler).
    pub async fn process_request(&self, request: SyncRequest) -> Result<SyncResult> {
        match request {
            SyncRequest::Full => self.sync_full().await,
            SyncRequest::Paths(paths) => self.sync_paths(paths).await,
            SyncRequest::Shutdown => Ok(SyncResult {
                files_synced: 0,
                files_failed: 0,
                conflicts_found: 0,
                duration: Duration::from_secs(0),
            }),
        }
    }

    /// Upload all staged changes.
    async fn upload_staged_changes(&self) -> (usize, usize, usize) {
        let mut synced = 0;
        let mut failed = 0;
        let mut conflicts = 0;

        let change_ids: Vec<String> = {
            let staging = self.staging.read().await;
            staging.all_changes().map(|c| c.id.clone()).collect()
        };

        for change_id in change_ids {
            let change = {
                let staging = self.staging.read().await;
                staging.get_change(&change_id).cloned()
            };

            let Some(change) = change else {
                continue;
            };

            debug!("Processing staged change: {} for {}", change_id, change.vault_path);

            match change.change_type {
                ChangeType::Create | ChangeType::Update => {
                    match self.upload_staged_file(&change_id, &change.vault_path).await {
                        Ok(has_conflict) => {
                            if has_conflict {
                                conflicts += 1;
                            } else {
                                synced += 1;
                                // Commit the change
                                if let Err(e) = self.staging.write().await.commit(&change_id).await {
                                    warn!("Failed to commit staged change: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to upload {}: {}", change.vault_path, e);
                            failed += 1;
                        }
                    }
                }
                ChangeType::Delete => {
                    match self.delete_remote_file(&change.vault_path).await {
                        Ok(_) => {
                            synced += 1;
                            if let Err(e) = self.staging.write().await.commit(&change_id).await {
                                warn!("Failed to commit staged change: {}", e);
                            }
                        }
                        Err(e) => {
                            error!("Failed to delete {}: {}", change.vault_path, e);
                            failed += 1;
                        }
                    }
                }
            }
        }

        (synced, failed, conflicts)
    }

    /// Upload a single staged file.
    async fn upload_staged_file(&self, change_id: &str, path: &VaultPath) -> Result<bool> {
        let data = {
            let staging = self.staging.read().await;
            staging.get_staged_data(change_id).await?
        };

        // Check for conflicts first
        let local_entry = {
            let state = self.state.read().await;
            state.get(path).cloned()
        };

        if let Some(ref entry) = local_entry {
            // Check if remote has changed
            let provider = self.provider.clone();
            let path_clone = path.clone();

            let remote_metadata = self
                .retry_executor
                .execute(|| {
                    let p = provider.clone();
                    let path = path_clone.clone();
                    async move { p.metadata(&path).await }
                })
                .await;

            if let Ok(remote) = remote_metadata {
                if self.conflict_resolver.detect_conflict(
                    entry.local_etag.as_deref(),
                    remote.etag.as_deref(),
                    entry.remote_etag.as_deref(),
                ) {
                    // Conflict detected
                    let conflict_info =
                        ConflictInfo::from_entry_and_remote(entry, &remote)?;

                    if self.config.auto_resolve_conflicts {
                        let result = self
                            .conflict_resolver
                            .resolve(
                                &conflict_info,
                                data,
                                self.provider.as_ref(),
                                self.config.conflict_strategy,
                            )
                            .await?;

                        self.handle_resolution_result(path, result).await?;
                        return Ok(false);
                    } else {
                        // Mark as conflicted
                        let mut state = self.state.write().await;
                        if let Some(entry) = state.get_mut(path) {
                            entry.mark_conflicted(remote.etag.clone(), remote.modified);
                        }
                        return Ok(true);
                    }
                }
            }
        }

        // No conflict, upload
        let provider = self.provider.clone();
        let path_clone = path.clone();

        let metadata = self
            .retry_executor
            .execute(move || {
                let p = provider.clone();
                let path = path_clone.clone();
                let d = data.clone();
                async move { p.upload(&path, d).await }
            })
            .await?;

        // Update sync state
        let mut state = self.state.write().await;
        if let Some(entry) = state.get_mut(path) {
            entry.mark_synced(metadata.etag.clone(), metadata.modified);
        } else {
            state.insert(SyncEntry::new_synced(
                path.to_string(),
                metadata.etag,
                metadata.modified,
            ));
        }

        Ok(false)
    }

    /// Delete a file from remote storage.
    async fn delete_remote_file(&self, path: &VaultPath) -> Result<()> {
        let provider = self.provider.clone();
        let path_clone = path.clone();

        self.retry_executor
            .execute(move || {
                let p = provider.clone();
                let path = path_clone.clone();
                async move { p.delete(&path).await }
            })
            .await?;

        // Remove from sync state
        let mut state = self.state.write().await;
        state.remove(path);

        Ok(())
    }

    /// Check remote for changes.
    async fn check_remote_changes(&self) -> Result<usize> {
        let mut conflicts = 0;

        // Get list of known paths
        let paths: Vec<String> = {
            let state = self.state.read().await;
            state.paths()
        };

        for path_str in paths {
            let path = VaultPath::parse(&path_str)?;
            let provider = self.provider.clone();
            let path_clone = path.clone();

            let remote_result = self
                .retry_executor
                .execute(move || {
                    let p = provider.clone();
                    let path = path_clone.clone();
                    async move { p.metadata(&path).await }
                })
                .await;

            if let Ok(remote) = remote_result {
                let mut state = self.state.write().await;
                if let Some(entry) = state.get_mut(&path) {
                    if entry.remote_etag != remote.etag {
                        entry.mark_remote_modified(remote.etag.clone(), remote.modified);
                        if entry.status == SyncStatus::Conflicted {
                            conflicts += 1;
                        }
                    }
                }
            }
        }

        Ok(conflicts)
    }

    /// Download remote changes to local.
    async fn download_remote_changes(&self) -> (usize, usize) {
        let mut synced = 0;
        let mut failed = 0;

        let entries: Vec<(String, SyncStatus)> = {
            let state = self.state.read().await;
            state
                .entries()
                .filter(|e| e.status == SyncStatus::RemoteModified)
                .map(|e| (e.path.clone(), e.status))
                .collect()
        };

        for (path_str, _) in entries {
            let path = match VaultPath::parse(&path_str) {
                Ok(p) => p,
                Err(_) => {
                    failed += 1;
                    continue;
                }
            };

            let provider = self.provider.clone();
            let path_clone = path.clone();

            let download_result = self
                .retry_executor
                .execute(move || {
                    let p = provider.clone();
                    let path = path_clone.clone();
                    async move { p.download(&path).await }
                })
                .await;

            match download_result {
                Ok(_data) => {
                    // In a real implementation, we would write this to the vault
                    // For now, just update the state
                    let provider = self.provider.clone();
                    let path_clone = path.clone();

                    if let Ok(metadata) = self
                        .retry_executor
                        .execute(move || {
                            let p = provider.clone();
                            let path = path_clone.clone();
                            async move { p.metadata(&path).await }
                        })
                        .await
                    {
                        let mut state = self.state.write().await;
                        if let Some(entry) = state.get_mut(&path) {
                            entry.mark_synced(metadata.etag, metadata.modified);
                        }
                    }
                    synced += 1;
                }
                Err(e) => {
                    error!("Failed to download {}: {}", path_str, e);
                    failed += 1;
                }
            }
        }

        (synced, failed)
    }

    /// Sync a single path.
    async fn sync_single_path(&self, path: &VaultPath) -> Result<SingleSyncResult> {
        let change_ids: Vec<String> = {
            let staging = self.staging.read().await;
            staging.changes_for_path(path).iter().map(|c| c.id.clone()).collect()
        };

        if !change_ids.is_empty() {
            // Has local changes, upload
            for change_id in change_ids {
                let has_conflict = self.upload_staged_file(&change_id, path).await?;
                if has_conflict {
                    return Ok(SingleSyncResult { has_conflict: true });
                }
                self.staging.write().await.commit(&change_id).await?;
            }
        } else {
            // Check remote for updates
            let provider = self.provider.clone();
            let path_clone = path.clone();

            let remote_metadata = self
                .retry_executor
                .execute(move || {
                    let p = provider.clone();
                    let path = path_clone.clone();
                    async move { p.metadata(&path).await }
                })
                .await?;

            let mut state = self.state.write().await;
            if let Some(entry) = state.get_mut(path) {
                if entry.remote_etag != remote_metadata.etag {
                    entry.mark_synced(remote_metadata.etag, remote_metadata.modified);
                }
            }
        }

        Ok(SingleSyncResult { has_conflict: false })
    }

    /// Handle the result of conflict resolution.
    async fn handle_resolution_result(
        &self,
        path: &VaultPath,
        result: ResolutionResult,
    ) -> Result<()> {
        let mut state = self.state.write().await;

        match result {
            ResolutionResult::UsedLocal { new_remote_etag } => {
                if let Some(entry) = state.get_mut(path) {
                    entry.mark_synced(new_remote_etag, chrono::Utc::now());
                }
            }
            ResolutionResult::UsedRemote { new_local_etag } => {
                if let Some(entry) = state.get_mut(path) {
                    entry.mark_synced(new_local_etag, chrono::Utc::now());
                }
            }
            ResolutionResult::KeptBoth {
                original_path,
                renamed_path,
                remote_etag,
            } => {
                // Update original path to synced with remote
                if let Some(entry) = state.get_mut(&original_path) {
                    entry.mark_synced(remote_etag, chrono::Utc::now());
                }
                // Add entry for renamed file
                let new_etag = Some(uuid::Uuid::new_v4().to_string());
                state.insert(SyncEntry::new_synced(
                    renamed_path.to_string(),
                    new_etag,
                    chrono::Utc::now(),
                ));
            }
            ResolutionResult::Pending => {
                // Nothing to do, conflict remains
            }
        }

        Ok(())
    }

    /// Get conflicts that need resolution.
    pub async fn get_conflicts(&self) -> Vec<VaultPath> {
        let state = self.state.read().await;
        state
            .entries_with_status(SyncStatus::Conflicted)
            .iter()
            .filter_map(|e| VaultPath::parse(&e.path).ok())
            .collect()
    }

    /// Manually resolve a conflict.
    pub async fn resolve_conflict(
        &self,
        path: &VaultPath,
        local_data: Vec<u8>,
        strategy: ConflictStrategy,
    ) -> Result<()> {
        let entry = {
            let state = self.state.read().await;
            state.get(path).cloned()
        };

        let Some(entry) = entry else {
            return Err(Error::NotFound(format!("No sync entry for {}", path)));
        };

        if entry.status != SyncStatus::Conflicted {
            return Err(Error::InvalidInput("Path is not in conflict".to_string()));
        }

        let remote_metadata = self.provider.metadata(path).await?;
        let conflict_info = ConflictInfo::from_entry_and_remote(&entry, &remote_metadata)?;

        let result = self
            .conflict_resolver
            .resolve(&conflict_info, local_data, self.provider.as_ref(), strategy)
            .await?;

        self.handle_resolution_result(path, result).await
    }
}

/// Result of syncing a single path.
struct SingleSyncResult {
    has_conflict: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests would require a mock StorageProvider
    // See integration tests for full testing
}
