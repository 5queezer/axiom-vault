//! Core sync engine that orchestrates all sync operations.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

use axiomvault_common::{Error, Result, VaultPath};
use axiomvault_storage::StorageProvider;

use crate::conflict::{ConflictInfo, ConflictResolver, ConflictStrategy, ResolutionResult};
use crate::mapping::{IdentityPathMapper, SyncPathMapper};
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

async fn persist_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|e| Error::Serialization(e.to_string()))?;
    let temp = path.with_extension(format!("json.tmp.{}", uuid::Uuid::new_v4()));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .await?;
    use tokio::io::AsyncWriteExt;
    if let Err(error) = file.write_all(&bytes).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error.into());
    }
    if let Err(error) = file.sync_all().await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error.into());
    }
    drop(file);
    if let Err(error) = tokio::fs::rename(&temp, path).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error.into());
    }
    Ok(())
}

/// Main sync engine for coordinating a remote provider and explicit local persistence.
pub struct SyncEngine<R: StorageProvider + ?Sized, L: StorageProvider + ?Sized = R> {
    /// Provider used for remote transport.
    remote: Arc<R>,
    /// Provider used as the durable local destination for downloads.
    local: Arc<L>,
    /// Maps canonical local paths to provider-specific remote paths.
    path_mapper: Arc<dyn SyncPathMapper>,
    /// Directory containing staging, state, and configuration files.
    state_dir: PathBuf,
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
    /// Guard to prevent concurrent sync operations.
    sync_lock: Arc<Mutex<()>>,
}

impl<P: StorageProvider + ?Sized + 'static> SyncEngine<P, P> {
    /// Backward-compatible single-provider constructor.
    ///
    /// This preserves the historical API by using the same provider for remote
    /// transport and local persistence. New production call sites should pass
    /// distinct providers through [`Self::from_arcs`].
    pub async fn from_arc(
        provider: Arc<P>,
        state_dir: impl AsRef<Path>,
        config: SyncConfig,
    ) -> Result<Self> {
        Self::from_arcs(provider.clone(), provider, state_dir, config).await
    }
}

impl<R: StorageProvider + ?Sized + 'static, L: StorageProvider + ?Sized + 'static>
    SyncEngine<R, L>
{
    /// Create an engine with identity path mapping.
    pub async fn from_arcs(
        remote: Arc<R>,
        local: Arc<L>,
        state_dir: impl AsRef<Path>,
        config: SyncConfig,
    ) -> Result<Self> {
        Self::from_arcs_with_mapper(
            remote,
            local,
            state_dir,
            config,
            Arc::new(IdentityPathMapper),
        )
        .await
    }

    /// Create an engine with an explicit provider path mapping.
    pub async fn from_arcs_with_mapper(
        remote: Arc<R>,
        local: Arc<L>,
        state_dir: impl AsRef<Path>,
        supplied_config: SyncConfig,
        path_mapper: Arc<dyn SyncPathMapper>,
    ) -> Result<Self> {
        let state_dir = state_dir.as_ref().to_path_buf();
        tokio::fs::create_dir_all(&state_dir).await?;
        let config_path = state_dir.join("sync_config.json");
        let config = if config_path.exists() {
            let bytes = tokio::fs::read(&config_path).await?;
            serde_json::from_slice(&bytes).map_err(|e| Error::Serialization(e.to_string()))?
        } else {
            persist_json_atomic(&config_path, &supplied_config).await?;
            supplied_config
        };
        let staging = StagingArea::new(&state_dir).await?;
        let state_path = state_dir.join("sync_state.json");
        let mut state = if state_path.exists() {
            let bytes = tokio::fs::read(&state_path).await?;
            serde_json::from_slice(&bytes).map_err(|e| Error::Serialization(e.to_string()))?
        } else {
            SyncState::new()
        };

        // The staging registry is authoritative for interrupted local changes.
        // Rebuild any missing state entries before persisting the loaded state.
        for change in staging.all_changes() {
            if state.get(&change.vault_path).is_none() {
                state.insert(SyncEntry::new_local(change.vault_path.to_string(), None));
            }
        }
        persist_json_atomic(&state_path, &state).await?;

        let retry_config = RetryConfig::new(config.max_retries);
        let conflict_resolver = ConflictResolver::new(config.conflict_strategy);
        Ok(Self {
            remote,
            local,
            path_mapper,
            state_dir,
            state: Arc::new(RwLock::new(state)),
            staging: Arc::new(RwLock::new(staging)),
            conflict_resolver: Arc::new(conflict_resolver),
            retry_executor: Arc::new(RetryExecutor::new(retry_config)),
            scheduler: None,
            config,
            sync_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Return the effective persisted configuration.
    pub fn config(&self) -> &SyncConfig {
        &self.config
    }

    async fn persist_state(&self) -> Result<()> {
        let snapshot = self.state.read().await.clone();
        persist_json_atomic(&self.state_dir.join("sync_state.json"), &snapshot).await
    }

    fn remote_path(&self, local_path: &VaultPath) -> Result<VaultPath> {
        self.path_mapper.local_to_remote(local_path)
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
        drop(state);
        self.persist_state().await?;

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
        drop(state);
        self.persist_state().await?;

        Ok(change_id)
    }

    /// Perform a full sync of all staged changes and fetch remote updates.
    ///
    /// Uses a mutex to prevent concurrent sync operations from racing.
    pub async fn sync_full(&self) -> Result<SyncResult> {
        // Acquire sync lock — a second concurrent call blocks here instead of racing
        let _guard = self.sync_lock.lock().await;

        let start = Instant::now();
        let mut files_synced = 0;
        let mut files_failed = 0;
        let mut conflicts_found = 0;
        let mut pending_persistence = 0;

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
        pending_persistence += download_result.2;

        {
            let mut state = self.state.write().await;
            state.sync_in_progress = false;
            state.last_full_sync = Some(chrono::Utc::now());
        }

        let duration = start.elapsed();
        info!(
            "Full sync completed in {:?}: {} synced, {} failed, {} conflicts, {} pending persistence",
            duration, files_synced, files_failed, conflicts_found, pending_persistence
        );

        Ok(SyncResult {
            files_synced,
            files_failed,
            conflicts_found,
            pending_persistence,
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
                    warn!("Invalid path: {}", e);
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
                    error!("Failed to sync path: {}", e);
                    files_failed += 1;
                }
            }
        }

        let duration = start.elapsed();
        Ok(SyncResult {
            files_synced,
            files_failed,
            conflicts_found,
            pending_persistence: 0,
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
                pending_persistence: 0,
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

            debug!("Processing staged change: {}", change_id);

            match change.change_type {
                ChangeType::Create | ChangeType::Update => {
                    match self
                        .upload_staged_file(&change_id, &change.vault_path)
                        .await
                    {
                        Ok(has_conflict) => {
                            if has_conflict {
                                conflicts += 1;
                            } else {
                                synced += 1;
                                // Commit the change
                                if let Err(e) = self.staging.write().await.commit(&change_id).await
                                {
                                    warn!("Failed to commit staged change: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to upload staged file: {}", e);
                            failed += 1;
                        }
                    }
                }
                ChangeType::Delete => match self.delete_remote_file(&change.vault_path).await {
                    Ok(_) => {
                        synced += 1;
                        if let Err(e) = self.staging.write().await.commit(&change_id).await {
                            warn!("Failed to commit staged change: {}", e);
                        }
                    }
                    Err(e) => {
                        error!("Failed to delete remote file: {}", e);
                        failed += 1;
                    }
                },
            }
        }

        (synced, failed, conflicts)
    }

    /// Upload a single staged file.
    async fn upload_staged_file(&self, change_id: &str, path: &VaultPath) -> Result<bool> {
        let remote_path = self.remote_path(path)?;
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
            let provider = self.remote.clone();
            let path_clone = remote_path.clone();

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
                    // Conflict detection is based on local state, but remote
                    // operations must use the mapped provider path.
                    let mut conflict_info = ConflictInfo::from_entry_and_remote(entry, &remote)?;
                    conflict_info.path = remote_path.clone();

                    if self.config.auto_resolve_conflicts {
                        let result = self
                            .conflict_resolver
                            .resolve(
                                &conflict_info,
                                data,
                                self.remote.as_ref(),
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
        let provider = self.remote.clone();
        let path_clone = remote_path;

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
        drop(state);
        self.persist_state().await?;

        Ok(false)
    }

    /// Delete a file from remote storage.
    async fn delete_remote_file(&self, path: &VaultPath) -> Result<()> {
        let provider = self.remote.clone();
        let path_clone = self.remote_path(path)?;

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
        drop(state);
        self.persist_state().await?;

        Ok(())
    }

    /// Discover remote objects, mark changed objects for download, and apply
    /// remote deletions to the local persistence provider.
    async fn check_remote_changes(&self) -> Result<usize> {
        let mut remote_files = Vec::new();
        let mut directories = vec![self.path_mapper.remote_root()];
        while let Some(directory) = directories.pop() {
            let children = self.remote.list(&directory).await?;
            for metadata in children {
                let remote_path = directory.join(&metadata.name)?;
                if metadata.is_directory {
                    directories.push(remote_path);
                } else {
                    remote_files.push((remote_path, metadata));
                }
            }
        }

        let mut seen_local_paths = HashSet::new();
        let mut conflicts = 0;
        {
            let mut state = self.state.write().await;
            for (remote_path, metadata) in remote_files {
                let local_path = self.path_mapper.remote_to_local(&remote_path)?;
                seen_local_paths.insert(local_path.to_string());
                match state.get_mut(&local_path) {
                    Some(entry) if entry.remote_etag != metadata.etag => {
                        entry.mark_remote_modified(metadata.etag.clone(), metadata.modified);
                        if entry.status == SyncStatus::Conflicted {
                            conflicts += 1;
                        }
                    }
                    Some(_) => {}
                    None => {
                        let mut entry = SyncEntry::new_local(local_path.to_string(), None);
                        entry.remote_etag = metadata.etag;
                        entry.remote_modified = Some(metadata.modified);
                        entry.status = SyncStatus::RemoteModified;
                        state.insert(entry);
                    }
                }
            }
        }

        let deleted_paths: Vec<VaultPath> = {
            let state = self.state.read().await;
            state
                .entries()
                .filter(|entry| {
                    entry.status == SyncStatus::Synced && !seen_local_paths.contains(&entry.path)
                })
                .filter_map(|entry| VaultPath::parse(&entry.path).ok())
                .collect()
        };
        for local_path in deleted_paths {
            if self.local.exists(&local_path).await? {
                self.local.delete(&local_path).await?;
            }
            self.state.write().await.remove(&local_path);
        }
        self.persist_state().await?;
        Ok(conflicts)
    }

    /// Download and durably persist every remotely modified object.
    ///
    /// State and remote ETags advance only after `replace_atomic` succeeds.
    async fn download_remote_changes(&self) -> (usize, usize, usize) {
        let mut synced = 0;
        let mut failed = 0;
        let entries: Vec<String> = {
            let state = self.state.read().await;
            state
                .entries()
                .filter(|entry| entry.status == SyncStatus::RemoteModified)
                .map(|entry| entry.path.clone())
                .collect()
        };

        for path_string in entries {
            let local_path = match VaultPath::parse(&path_string) {
                Ok(path) => path,
                Err(_) => {
                    failed += 1;
                    continue;
                }
            };
            let remote_path = match self.remote_path(&local_path) {
                Ok(path) => path,
                Err(_) => {
                    failed += 1;
                    continue;
                }
            };
            let remote = self.remote.clone();
            let download_path = remote_path.clone();
            let data = self
                .retry_executor
                .execute(move || {
                    let provider = remote.clone();
                    let path = download_path.clone();
                    async move { provider.download(&path).await }
                })
                .await;

            let result = match data {
                Ok(data) => self.local.replace_atomic(&local_path, data).await,
                Err(error) => Err(error),
            };
            match result {
                Ok(_) => match self.remote.metadata(&remote_path).await {
                    Ok(metadata) => {
                        let mut state = self.state.write().await;
                        if let Some(entry) = state.get_mut(&local_path) {
                            entry.mark_synced(metadata.etag, metadata.modified);
                        }
                        drop(state);
                        if let Err(error) = self.persist_state().await {
                            error!("Failed to persist sync state: {}", error);
                            failed += 1;
                        } else {
                            synced += 1;
                        }
                    }
                    Err(error) => {
                        error!(
                            "Failed to read remote metadata after persistence: {}",
                            error
                        );
                        failed += 1;
                    }
                },
                Err(error) => {
                    error!("Failed to persist downloaded file: {}", error);
                    failed += 1;
                }
            }
        }
        (synced, failed, 0)
    }

    /// Sync a single path.
    async fn sync_single_path(&self, path: &VaultPath) -> Result<SingleSyncResult> {
        let change_ids: Vec<String> = {
            let staging = self.staging.read().await;
            staging
                .changes_for_path(path)
                .iter()
                .map(|c| c.id.clone())
                .collect()
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
            let remote_path = self.remote_path(path)?;
            let remote_metadata = self.remote.metadata(&remote_path).await?;
            let changed = {
                let state = self.state.read().await;
                state
                    .get(path)
                    .is_none_or(|entry| entry.remote_etag != remote_metadata.etag)
            };
            if changed {
                let data = self.remote.download(&remote_path).await?;
                self.local.replace_atomic(path, data).await?;
                let mut state = self.state.write().await;
                if let Some(entry) = state.get_mut(path) {
                    entry.mark_synced(remote_metadata.etag, remote_metadata.modified);
                } else {
                    state.insert(SyncEntry::new_synced(
                        path.to_string(),
                        remote_metadata.etag,
                        remote_metadata.modified,
                    ));
                }
                drop(state);
                self.persist_state().await?;
            }
        }

        Ok(SingleSyncResult {
            has_conflict: false,
        })
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
        drop(state);
        self.persist_state().await?;

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

        let remote_metadata = self.remote.metadata(path).await?;
        let conflict_info = ConflictInfo::from_entry_and_remote(&entry, &remote_metadata)?;

        let result = self
            .conflict_resolver
            .resolve(&conflict_info, local_data, self.remote.as_ref(), strategy)
            .await?;

        self.handle_resolution_result(path, result).await
    }
}

/// Result of syncing a single path.
struct SingleSyncResult {
    has_conflict: bool,
}
