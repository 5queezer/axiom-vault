//! Recovery and rebuild of missing shards.
//!
//! When a backend recovers after failure or is replaced, the `RaidRebuilder`
//! reconstructs missing data on the target backend. In mirror mode it copies
//! whole chunks from a healthy peer; in erasure mode it reconstructs the
//! specific missing shard from the remaining shards via Reed-Solomon.
//!
//! Rebuilds are incremental (existing data is skipped) and resumable
//! (re-running the rebuild picks up where it left off). A persistent
//! checkpoint is saved periodically so that a crashed rebuild can resume
//! from where it left off without re-transferring completed chunks.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::composite::{CompositeStorageProvider, RaidMode};
use crate::provider::StorageProvider;
use axiomvault_common::{Error, Result, VaultPath};

/// Well-known path for the rebuild checkpoint file on the target backend.
const CHECKPOINT_PATH: &str = ".axiomvault/rebuild_checkpoint.json";

/// Configuration for a rebuild operation.
#[derive(Debug, Clone)]
pub struct RebuildConfig {
    /// Maximum number of concurrent transfers during rebuild. Default: 4.
    pub concurrency: usize,
    /// Save a checkpoint every N completed chunks. Default: 10.
    /// Set to 0 to disable periodic checkpointing (checkpoint is still
    /// deleted on successful completion).
    pub checkpoint_interval: usize,
}

impl Default for RebuildConfig {
    fn default() -> Self {
        Self {
            concurrency: 4,
            checkpoint_interval: 10,
        }
    }
}

/// Persistent checkpoint for resumable rebuilds.
///
/// Serialized to JSON and stored on the target backend so that a crashed
/// rebuild can skip already-completed chunks on restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebuildCheckpoint {
    /// Index of the target backend being rebuilt.
    pub target_index: usize,
    /// Set of chunk paths that have been successfully rebuilt or confirmed present.
    pub completed_paths: HashSet<String>,
    /// Timestamp when this checkpoint was last saved.
    pub saved_at: DateTime<Utc>,
    /// The RAID mode active when this checkpoint was created.
    pub mode: RaidMode,
}

impl RebuildCheckpoint {
    /// Create a new empty checkpoint.
    fn new(target_index: usize, mode: RaidMode) -> Self {
        Self {
            target_index,
            completed_paths: HashSet::new(),
            saved_at: Utc::now(),
            mode,
        }
    }

    /// Load a checkpoint from the target backend, if one exists.
    ///
    /// Returns `None` if no checkpoint is found or it cannot be parsed.
    async fn load(
        backend: &dyn StorageProvider,
        target_index: usize,
        mode: RaidMode,
    ) -> Option<Self> {
        let path = match VaultPath::parse(CHECKPOINT_PATH) {
            Ok(p) => p,
            Err(_) => return None,
        };
        let data = match backend.download(&path).await {
            Ok(d) => d,
            Err(_) => return None,
        };
        let ckpt: Self = match serde_json::from_slice(&data) {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "Failed to parse rebuild checkpoint, starting fresh");
                return None;
            }
        };
        // Only resume if the checkpoint matches this rebuild's parameters.
        if ckpt.target_index != target_index || ckpt.mode != mode {
            warn!("Rebuild checkpoint does not match current parameters, starting fresh");
            return None;
        }
        Some(ckpt)
    }

    /// Save this checkpoint to the target backend.
    async fn save(&mut self, backend: &dyn StorageProvider) -> Result<()> {
        self.saved_at = Utc::now();
        let data =
            serde_json::to_vec_pretty(self).map_err(|e| Error::Serialization(e.to_string()))?;
        let path = VaultPath::parse(CHECKPOINT_PATH)?;
        // Ensure the .axiomvault directory exists.
        let dir = VaultPath::parse(".axiomvault")?;
        if !backend.exists(&dir).await.unwrap_or(false) {
            let _ = backend.create_dir(&dir).await;
        }
        backend.upload(&path, data).await?;
        Ok(())
    }

    /// Delete the checkpoint from the target backend (called on successful completion).
    async fn delete(backend: &dyn StorageProvider) -> Result<()> {
        let path = VaultPath::parse(CHECKPOINT_PATH)?;
        if backend.exists(&path).await.unwrap_or(false) {
            backend.delete(&path).await?;
        }
        Ok(())
    }
}

/// Progress of an ongoing rebuild.
#[derive(Debug, Clone)]
pub struct RebuildProgress {
    /// Total chunks/shards to process.
    pub total: usize,
    /// Successfully rebuilt.
    pub completed: usize,
    /// Skipped because already present on target.
    pub skipped: usize,
    /// Failed to rebuild.
    pub failed: usize,
    /// When the rebuild started.
    pub started_at: Instant,
}

impl RebuildProgress {
    fn new(total: usize) -> Self {
        Self {
            total,
            completed: 0,
            skipped: 0,
            failed: 0,
            started_at: Instant::now(),
        }
    }

    /// Percentage complete (0.0 – 100.0).
    pub fn percentage(&self) -> f64 {
        if self.total == 0 {
            return 100.0;
        }
        let done = self.completed + self.skipped + self.failed;
        (done as f64 / self.total as f64) * 100.0
    }

    /// Number of items remaining.
    pub fn remaining(&self) -> usize {
        let done = self.completed + self.skipped + self.failed;
        self.total.saturating_sub(done)
    }

    /// Elapsed time since the rebuild started.
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Estimated time to completion based on current throughput.
    pub fn eta(&self) -> Option<Duration> {
        let done = self.completed + self.skipped + self.failed;
        if done == 0 {
            return None;
        }
        let elapsed = self.started_at.elapsed();
        let per_item = elapsed / done as u32;
        Some(per_item * self.remaining() as u32)
    }
}

/// Summary returned when a rebuild completes.
#[derive(Debug, Clone)]
pub struct RebuildResult {
    /// Number of chunks/shards successfully rebuilt.
    pub rebuilt: usize,
    /// Number of chunks/shards skipped (already present).
    pub skipped: usize,
    /// Number of chunks/shards that failed to rebuild.
    pub failed: usize,
    /// Total wall-clock time for the rebuild.
    pub elapsed: Duration,
}

/// Rebuilds missing data on a target backend from healthy peers.
pub struct RaidRebuilder<'a> {
    composite: &'a CompositeStorageProvider,
    target_index: usize,
    config: RebuildConfig,
    progress: Arc<RwLock<RebuildProgress>>,
    /// Optional push-based progress channel. When set, a snapshot of
    /// `RebuildProgress` is sent after each chunk is processed.
    progress_tx: Option<tokio::sync::mpsc::Sender<RebuildProgress>>,
}

impl<'a> RaidRebuilder<'a> {
    /// Create a new rebuilder targeting the backend at `target_index`.
    pub fn new(
        composite: &'a CompositeStorageProvider,
        target_index: usize,
        config: RebuildConfig,
    ) -> Result<Self> {
        if config.concurrency == 0 {
            return Err(Error::InvalidInput(
                "rebuild concurrency must be at least 1".to_string(),
            ));
        }
        if target_index >= composite.backend_count() {
            return Err(Error::InvalidInput(format!(
                "target_index {} out of range (backend count: {})",
                target_index,
                composite.backend_count()
            )));
        }
        Ok(Self {
            composite,
            target_index,
            config,
            progress: Arc::new(RwLock::new(RebuildProgress::new(0))),
            progress_tx: None,
        })
    }

    /// Attach a push-based progress channel.
    ///
    /// When set, a snapshot of `RebuildProgress` is sent through `tx` after
    /// each chunk is processed (rebuilt, skipped, or failed). The existing
    /// poll-based `progress()` method continues to work regardless.
    pub fn with_progress_channel(mut self, tx: tokio::sync::mpsc::Sender<RebuildProgress>) -> Self {
        self.progress_tx = Some(tx);
        self
    }

    /// Get a snapshot of current rebuild progress.
    pub async fn progress(&self) -> RebuildProgress {
        self.progress.read().await.clone()
    }

    /// Send a progress snapshot through the channel, if one is attached.
    async fn notify_progress(&self) {
        if let Some(tx) = &self.progress_tx {
            let snapshot = self.progress.read().await.clone();
            // Best-effort: drop the update if the receiver is full or gone.
            let _ = tx.try_send(snapshot);
        }
    }

    /// Run the rebuild, returning a summary when complete.
    pub async fn rebuild(&self) -> Result<RebuildResult> {
        // Ensure the shard map is loaded.
        self.composite.load_shard_map().await?;

        match self.composite.mode() {
            RaidMode::Mirror => self.rebuild_mirror().await,
            RaidMode::Erasure { .. } => self.rebuild_erasure().await,
        }
    }

    /// Mirror-mode rebuild: copy missing chunks from a healthy peer to the target.
    async fn rebuild_mirror(&self) -> Result<RebuildResult> {
        let shard_map = self.composite.get_shard_map().await;
        let backends = self.composite.backends();
        let target = &backends[self.target_index];
        let entries: Vec<(String, u64)> = shard_map
            .entries
            .iter()
            .map(|(path, entry)| (path.clone(), entry.original_size))
            .collect();

        let total = entries.len();
        {
            let mut p = self.progress.write().await;
            *p = RebuildProgress::new(total);
        }

        if total == 0 {
            return Ok(RebuildResult {
                rebuilt: 0,
                skipped: 0,
                failed: 0,
                elapsed: Duration::ZERO,
            });
        }

        let started_at = Instant::now();
        let progress = Arc::clone(&self.progress);
        let target_index = self.target_index;

        // Load any existing checkpoint for resume.
        let mut checkpoint =
            RebuildCheckpoint::load(target.as_ref(), target_index, RaidMode::Mirror)
                .await
                .unwrap_or_else(|| RebuildCheckpoint::new(target_index, RaidMode::Mirror));

        // Build a list of (path, source_backend_index) for chunks that are missing on target.
        let mut work_items: Vec<(String, Vec<usize>)> = Vec::new();
        let mut skipped = 0usize;

        for (path, _) in &entries {
            // Skip paths already completed in a previous run's checkpoint.
            if checkpoint.completed_paths.contains(path) {
                skipped += 1;
                continue;
            }

            let vault_path = VaultPath::parse(path)?;
            match target.exists(&vault_path).await {
                Ok(true) => {
                    skipped += 1;
                    checkpoint.completed_paths.insert(path.clone());
                }
                _ => {
                    // Collect all candidate backends that have this chunk.
                    let mut candidates = Vec::new();
                    if let Some(entry) = shard_map.get(path) {
                        for shard in entry.shards.values() {
                            if shard.shard_index == target_index {
                                continue;
                            }
                            let src = shard.shard_index;
                            if src < backends.len()
                                && backends[src].exists(&vault_path).await.unwrap_or(false)
                            {
                                candidates.push(src);
                            }
                        }
                    }
                    if candidates.is_empty() {
                        let mut p = progress.write().await;
                        p.failed += 1;
                        self.notify_progress().await;
                        warn!(
                            path = path.as_str(),
                            "Mirror rebuild: no source backend available"
                        );
                    } else {
                        work_items.push((path.clone(), candidates));
                    }
                }
            }
        }

        {
            let mut p = progress.write().await;
            p.skipped = skipped;
        }
        self.notify_progress().await;

        // Process work items with bounded concurrency.
        // We use a stream that yields (path, success) results so we can
        // checkpoint and send progress notifications incrementally.
        let composite = self.composite;
        let checkpoint = Arc::new(RwLock::new(checkpoint));
        let checkpoint_interval = self.config.checkpoint_interval;
        let items_since_checkpoint = Arc::new(RwLock::new(0usize));

        let results: Vec<(String, bool)> = stream::iter(work_items)
            .map(|(path, candidates)| {
                let progress = Arc::clone(&progress);
                async move {
                    let result =
                        Self::copy_chunk(composite.backends(), &candidates, target_index, &path)
                            .await;
                    let mut p = progress.write().await;
                    match result {
                        Ok(()) => {
                            p.completed += 1;
                            (path, true)
                        }
                        Err(e) => {
                            warn!(path = path.as_str(), error = %e, "Mirror rebuild: failed to copy chunk");
                            p.failed += 1;
                            (path, false)
                        }
                    }
                }
            })
            .buffer_unordered(self.config.concurrency)
            .then(|(path, ok)| {
                let checkpoint = Arc::clone(&checkpoint);
                let items_since = Arc::clone(&items_since_checkpoint);
                async move {
                    if ok {
                        checkpoint.write().await.completed_paths.insert(path.clone());
                    }
                    self.notify_progress().await;

                    // Periodic checkpoint save.
                    if checkpoint_interval > 0 {
                        let mut count = items_since.write().await;
                        *count += 1;
                        if *count >= checkpoint_interval {
                            *count = 0;
                            let mut ckpt = checkpoint.write().await;
                            let _ = ckpt.save(target.as_ref()).await;
                        }
                    }

                    (path, ok)
                }
            })
            .collect()
            .await;

        let rebuilt = results.iter().filter(|(_, ok)| *ok).count();

        // Update shard map to include target backend in rebuilt entries.
        // Separate lock acquisition from backend I/O to avoid holding the lock during async calls.
        let paths_to_check: Vec<String> = {
            let map = self.composite.shard_map_ref().read().await;
            map.entries
                .iter()
                .filter(|(_, e)| !e.shards.contains_key(&target_index))
                .map(|(p, _)| p.clone())
                .collect()
        };

        // Probe without holding the shard map lock.
        let mut confirmed_paths: Vec<String> = Vec::new();
        for path in paths_to_check {
            let vault_path = match VaultPath::parse(&path) {
                Ok(vp) => vp,
                Err(_) => continue,
            };
            if backends[target_index]
                .exists(&vault_path)
                .await
                .unwrap_or(false)
            {
                confirmed_paths.push(path);
            }
        }

        // Acquire write lock only to mutate the map.
        let mut map_dirty = false;
        if !confirmed_paths.is_empty() {
            let mut map = self.composite.shard_map_ref().write().await;
            for path in &confirmed_paths {
                if let Some(entry) = map.entries.get_mut(path) {
                    use std::collections::hash_map::Entry;
                    if let Entry::Vacant(slot) = entry.shards.entry(target_index) {
                        let backend_id =
                            format!("{}:{}", backends[target_index].name(), target_index);
                        slot.insert(crate::shard_map::ShardLocation {
                            shard_index: target_index,
                            backend_id,
                            backend_path: path.clone(),
                            is_parity: false,
                        });
                        entry.updated_at = chrono::Utc::now();
                        map_dirty = true;
                    }
                }
            }
            if map_dirty {
                map.version += 1;
                map.updated_at = chrono::Utc::now();
            }
        }

        if map_dirty {
            self.composite.save_shard_map().await?;
        }

        // Delete checkpoint on successful completion.
        let _ = RebuildCheckpoint::delete(target.as_ref()).await;

        let progress = self.progress.read().await;
        Ok(RebuildResult {
            rebuilt,
            skipped: progress.skipped,
            failed: progress.failed,
            elapsed: started_at.elapsed(),
        })
    }

    /// Copy a single chunk from one of the source backends to the target.
    /// Tries each candidate in order; returns Ok on first successful copy.
    async fn copy_chunk(
        backends: &[Arc<dyn StorageProvider>],
        source_indexes: &[usize],
        target_index: usize,
        path: &str,
    ) -> Result<()> {
        let vault_path = VaultPath::parse(path)?;
        let mut last_err = None;
        for &src in source_indexes {
            match backends[src].download(&vault_path).await {
                Ok(data) => {
                    backends[target_index].upload(&vault_path, data).await?;
                    return Ok(());
                }
                Err(e) => {
                    warn!(source = src, path, error = %e, "Mirror rebuild: download failed, trying next candidate");
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| {
            Error::Storage(format!("Mirror rebuild: no readable source for '{}'", path))
        }))
    }

    /// Erasure-mode rebuild: reconstruct missing shards from available peers.
    async fn rebuild_erasure(&self) -> Result<RebuildResult> {
        let shard_map = self.composite.get_shard_map().await;
        let backends = self.composite.backends();
        let (data_shards, parity_shards) = self.composite.erasure_params()?;
        let total_shards = data_shards + parity_shards;
        let mode = RaidMode::Erasure {
            data_shards,
            parity_shards,
        };

        // Collect entries where the target backend's shard is missing.
        let entries: Vec<(String, crate::shard_map::ChunkEntry)> = shard_map
            .entries
            .iter()
            .map(|(p, e)| (p.clone(), e.clone()))
            .collect();

        let total = entries.len();
        {
            let mut p = self.progress.write().await;
            *p = RebuildProgress::new(total);
        }

        if total == 0 {
            return Ok(RebuildResult {
                rebuilt: 0,
                skipped: 0,
                failed: 0,
                elapsed: Duration::ZERO,
            });
        }

        let started_at = Instant::now();
        let progress = Arc::clone(&self.progress);
        let target_index = self.target_index;
        let target = &backends[target_index];

        // Load any existing checkpoint for resume.
        let mut checkpoint = RebuildCheckpoint::load(target.as_ref(), target_index, mode)
            .await
            .unwrap_or_else(|| RebuildCheckpoint::new(target_index, mode));

        // Determine which entries need rebuilding.
        let mut work_items: Vec<(String, crate::shard_map::ChunkEntry)> = Vec::new();
        let mut skipped = 0usize;

        for (path, entry) in &entries {
            // Skip paths already completed in a previous run's checkpoint.
            if checkpoint.completed_paths.contains(path) {
                skipped += 1;
                continue;
            }

            let shard_path =
                CompositeStorageProvider::shard_path(&VaultPath::parse(path)?, target_index)?;
            match backends[target_index].exists(&shard_path).await {
                Ok(true) => {
                    skipped += 1;
                    checkpoint.completed_paths.insert(path.clone());
                }
                _ => {
                    work_items.push((path.clone(), entry.clone()));
                }
            }
        }

        {
            let mut p = progress.write().await;
            p.skipped = skipped;
        }
        self.notify_progress().await;

        // Process work items with bounded concurrency.
        let composite = self.composite;
        let checkpoint = Arc::new(RwLock::new(checkpoint));
        let checkpoint_interval = self.config.checkpoint_interval;
        let items_since_checkpoint = Arc::new(RwLock::new(0usize));

        let results: Vec<(String, bool)> = stream::iter(work_items)
            .map(|(path, entry)| {
                let progress = Arc::clone(&progress);
                async move {
                    let result = Self::rebuild_erasure_shard(
                        composite,
                        target_index,
                        &path,
                        entry.original_size,
                        data_shards,
                        total_shards,
                    )
                    .await;
                    let mut p = progress.write().await;
                    match result {
                        Ok(()) => {
                            p.completed += 1;
                            (path, true)
                        }
                        Err(e) => {
                            warn!(path = path.as_str(), error = %e, "Erasure rebuild: failed to reconstruct shard");
                            p.failed += 1;
                            (path, false)
                        }
                    }
                }
            })
            .buffer_unordered(self.config.concurrency)
            .then(|(path, ok)| {
                let checkpoint = Arc::clone(&checkpoint);
                let items_since = Arc::clone(&items_since_checkpoint);
                async move {
                    if ok {
                        checkpoint.write().await.completed_paths.insert(path.clone());
                    }
                    self.notify_progress().await;

                    // Periodic checkpoint save.
                    if checkpoint_interval > 0 {
                        let mut count = items_since.write().await;
                        *count += 1;
                        if *count >= checkpoint_interval {
                            *count = 0;
                            let mut ckpt = checkpoint.write().await;
                            let _ = ckpt.save(target.as_ref()).await;
                        }
                    }

                    (path, ok)
                }
            })
            .collect()
            .await;

        let rebuilt = results.iter().filter(|(_, ok)| *ok).count();

        // Update shard map entries for rebuilt shards.
        // Separate lock acquisition from backend I/O to avoid holding the lock during async calls.
        let paths_to_check: Vec<String> = {
            let map = self.composite.shard_map_ref().read().await;
            map.entries
                .iter()
                .filter(|(_, e)| !e.shards.contains_key(&target_index))
                .map(|(p, _)| p.clone())
                .collect()
        };

        // Probe without holding the shard map lock.
        let mut confirmed: Vec<(String, String)> = Vec::new(); // (path, shard_path_str)
        for path in paths_to_check {
            let vault_path = match VaultPath::parse(&path) {
                Ok(vp) => vp,
                Err(_) => continue,
            };
            let shard_path = match CompositeStorageProvider::shard_path(&vault_path, target_index) {
                Ok(sp) => sp,
                Err(_) => continue,
            };
            if backends[target_index]
                .exists(&shard_path)
                .await
                .unwrap_or(false)
            {
                confirmed.push((path, shard_path.to_string_path()));
            }
        }

        // Acquire write lock only to mutate the map.
        let mut map_dirty = false;
        if !confirmed.is_empty() {
            let mut map = self.composite.shard_map_ref().write().await;
            for (path, shard_path_str) in &confirmed {
                if let Some(entry) = map.entries.get_mut(path) {
                    use std::collections::hash_map::Entry;
                    if let Entry::Vacant(slot) = entry.shards.entry(target_index) {
                        let backend_id =
                            format!("{}:{}", backends[target_index].name(), target_index);
                        slot.insert(crate::shard_map::ShardLocation {
                            shard_index: target_index,
                            backend_id,
                            backend_path: shard_path_str.clone(),
                            is_parity: target_index >= data_shards,
                        });
                        entry.updated_at = chrono::Utc::now();
                        map_dirty = true;
                    }
                }
            }
            if map_dirty {
                map.version += 1;
                map.updated_at = chrono::Utc::now();
            }
        }

        if map_dirty {
            self.composite.save_shard_map().await?;
        }

        // Delete checkpoint on successful completion.
        let _ = RebuildCheckpoint::delete(target.as_ref()).await;

        let progress = self.progress.read().await;
        Ok(RebuildResult {
            rebuilt,
            skipped: progress.skipped,
            failed: progress.failed,
            elapsed: started_at.elapsed(),
        })
    }

    /// Reconstruct a single missing shard for an erasure-coded chunk.
    ///
    /// Downloads available shards from peers, reconstructs via Reed-Solomon,
    /// and uploads the missing shard (with CRC header) to the target backend.
    async fn rebuild_erasure_shard(
        composite: &CompositeStorageProvider,
        target_index: usize,
        path: &str,
        original_size: u64,
        data_shards: usize,
        total_shards: usize,
    ) -> Result<()> {
        let vault_path = VaultPath::parse(path)?;
        let backends = composite.backends();

        // Download all available shards from peer backends.
        let mut shard_opts: Vec<Option<Vec<u8>>> = vec![None; total_shards];
        let mut available = 0usize;

        for i in 0..total_shards {
            if i == target_index {
                continue; // This is the shard we need to rebuild.
            }
            let shard_path = CompositeStorageProvider::shard_path(&vault_path, i)?;
            match backends[i].download(&shard_path).await {
                Ok(payload) => {
                    if payload.len() < 12 {
                        warn!(shard = i, path, "Rebuild: shard too short, skipping");
                        continue;
                    }
                    let stored_crc = u32::from_le_bytes(payload[..4].try_into().unwrap());
                    let shard_data = &payload[12..];

                    // Verify CRC-32 integrity.
                    let computed_crc = crc32fast::hash(shard_data);
                    if stored_crc != computed_crc {
                        warn!(shard = i, path, "Rebuild: CRC mismatch, skipping shard");
                        continue;
                    }

                    shard_opts[i] = Some(shard_data.to_vec());
                    available += 1;
                }
                Err(e) => {
                    warn!(shard = i, path, error = %e, "Rebuild: failed to download shard");
                }
            }
        }

        if available < data_shards {
            return Err(Error::Storage(format!(
                "Rebuild: only {}/{} shards available for '{}', need {}",
                available, total_shards, path, data_shards
            )));
        }

        // Reconstruct the missing shard via Reed-Solomon.
        composite
            .reed_solomon()?
            .reconstruct(&mut shard_opts)
            .map_err(|e| Error::Storage(format!("Reed-Solomon reconstruct failed: {}", e)))?;

        let rebuilt_shard = shard_opts[target_index].as_ref().ok_or_else(|| {
            Error::Storage(format!(
                "Rebuild: reconstruction did not produce shard {} for '{}'",
                target_index, path
            ))
        })?;

        // Build the shard payload with CRC header.
        let crc = crc32fast::hash(rebuilt_shard);
        let mut payload = Vec::with_capacity(4 + 8 + rebuilt_shard.len());
        payload.extend_from_slice(&crc.to_le_bytes());
        payload.extend_from_slice(&original_size.to_le_bytes());
        payload.extend_from_slice(rebuilt_shard);

        // Upload to target backend.
        let shard_path = CompositeStorageProvider::shard_path(&vault_path, target_index)?;
        backends[target_index].upload(&shard_path, payload).await?;

        info!(
            shard = target_index,
            path, "Erasure rebuild: shard reconstructed and uploaded"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composite::CompositeConfig;
    use crate::memory::MemoryProvider;

    fn make_mirror_composite(
        n: usize,
    ) -> (CompositeStorageProvider, Vec<Arc<dyn StorageProvider>>) {
        let backends: Vec<Arc<dyn StorageProvider>> = (0..n)
            .map(|_| Arc::new(MemoryProvider::new()) as _)
            .collect();
        let config = CompositeConfig {
            mode: RaidMode::Mirror,
            health: Default::default(),
        };
        let composite =
            CompositeStorageProvider::new(backends.clone(), config).expect("mirror composite");
        (composite, backends)
    }

    fn make_erasure_composite(
        data: usize,
        parity: usize,
    ) -> (CompositeStorageProvider, Vec<Arc<dyn StorageProvider>>) {
        let n = data + parity;
        let backends: Vec<Arc<dyn StorageProvider>> = (0..n)
            .map(|_| Arc::new(MemoryProvider::new()) as _)
            .collect();
        let config = CompositeConfig {
            mode: RaidMode::Erasure {
                data_shards: data,
                parity_shards: parity,
            },
            health: Default::default(),
        };
        let composite =
            CompositeStorageProvider::new(backends.clone(), config).expect("erasure composite");
        (composite, backends)
    }

    // Helper: upload via the composite and return the data used.
    async fn upload_file(composite: &CompositeStorageProvider, path: &str, data: &[u8]) {
        let vp = VaultPath::parse(path).unwrap();
        composite.upload(&vp, data.to_vec()).await.unwrap();
    }

    // -- Mirror rebuild tests ------------------------------------------------

    #[tokio::test]
    async fn test_mirror_rebuild_restores_missing_chunk() {
        let (composite, backends) = make_mirror_composite(3);

        upload_file(&composite, "/file1.enc", b"hello world").await;

        // Verify all backends have the file.
        let vp = VaultPath::parse("/file1.enc").unwrap();
        for b in &backends {
            assert!(b.exists(&vp).await.unwrap());
        }

        // Delete from target backend (index 2).
        backends[2].delete(&vp).await.unwrap();
        assert!(!backends[2].exists(&vp).await.unwrap());

        // Rebuild.
        let rebuilder = RaidRebuilder::new(&composite, 2, RebuildConfig::default()).unwrap();
        let result = rebuilder.rebuild().await.unwrap();

        assert_eq!(result.rebuilt, 1);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.failed, 0);

        // Verify data restored.
        let restored = backends[2].download(&vp).await.unwrap();
        assert_eq!(restored, b"hello world");
    }

    #[tokio::test]
    async fn test_mirror_rebuild_skips_existing() {
        let (composite, _backends) = make_mirror_composite(3);

        upload_file(&composite, "/file1.enc", b"data1").await;

        // Nothing deleted — rebuild should skip everything.
        let rebuilder = RaidRebuilder::new(&composite, 2, RebuildConfig::default()).unwrap();
        let result = rebuilder.rebuild().await.unwrap();

        assert_eq!(result.rebuilt, 0);
        assert_eq!(result.skipped, 1);
        assert_eq!(result.failed, 0);
    }

    #[tokio::test]
    async fn test_mirror_rebuild_multiple_files() {
        let (composite, backends) = make_mirror_composite(3);

        upload_file(&composite, "/a.enc", b"aaa").await;
        upload_file(&composite, "/b.enc", b"bbb").await;
        upload_file(&composite, "/c.enc", b"ccc").await;

        // Delete two files from backend 1.
        let pa = VaultPath::parse("/a.enc").unwrap();
        let pc = VaultPath::parse("/c.enc").unwrap();
        backends[1].delete(&pa).await.unwrap();
        backends[1].delete(&pc).await.unwrap();

        let rebuilder = RaidRebuilder::new(&composite, 1, RebuildConfig::default()).unwrap();
        let result = rebuilder.rebuild().await.unwrap();

        assert_eq!(result.rebuilt, 2);
        assert_eq!(result.skipped, 1);
        assert_eq!(result.failed, 0);

        // Verify restored data.
        assert_eq!(backends[1].download(&pa).await.unwrap(), b"aaa");
        assert_eq!(backends[1].download(&pc).await.unwrap(), b"ccc");
    }

    #[tokio::test]
    async fn test_mirror_rebuild_noop_empty_map() {
        let (composite, _backends) = make_mirror_composite(3);

        let rebuilder = RaidRebuilder::new(&composite, 0, RebuildConfig::default()).unwrap();
        let result = rebuilder.rebuild().await.unwrap();

        assert_eq!(result.rebuilt, 0);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.failed, 0);
    }

    // -- Erasure rebuild tests -----------------------------------------------

    #[tokio::test]
    async fn test_erasure_rebuild_restores_missing_shard() {
        let (composite, backends) = make_erasure_composite(3, 2);

        upload_file(&composite, "/file1.enc", b"erasure test data here").await;

        // Determine shard path for backend 2.
        let shard_path =
            CompositeStorageProvider::shard_path(&VaultPath::parse("/file1.enc").unwrap(), 2)
                .unwrap();

        // Verify shard exists.
        assert!(backends[2].exists(&shard_path).await.unwrap());

        // Delete shard from backend 2.
        backends[2].delete(&shard_path).await.unwrap();
        assert!(!backends[2].exists(&shard_path).await.unwrap());

        // Rebuild.
        let rebuilder = RaidRebuilder::new(&composite, 2, RebuildConfig::default()).unwrap();
        let result = rebuilder.rebuild().await.unwrap();

        assert_eq!(result.rebuilt, 1);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.failed, 0);

        // Verify shard was restored.
        assert!(backends[2].exists(&shard_path).await.unwrap());

        // Verify full data can still be downloaded.
        let vp = VaultPath::parse("/file1.enc").unwrap();
        let downloaded = composite.download(&vp).await.unwrap();
        assert_eq!(downloaded, b"erasure test data here");
    }

    #[tokio::test]
    async fn test_erasure_rebuild_skips_existing_shard() {
        let (composite, _backends) = make_erasure_composite(3, 2);

        upload_file(&composite, "/file1.enc", b"data").await;

        // Nothing deleted — should skip.
        let rebuilder = RaidRebuilder::new(&composite, 2, RebuildConfig::default()).unwrap();
        let result = rebuilder.rebuild().await.unwrap();

        assert_eq!(result.rebuilt, 0);
        assert_eq!(result.skipped, 1);
        assert_eq!(result.failed, 0);
    }

    #[tokio::test]
    async fn test_erasure_rebuild_parity_shard() {
        // Rebuild a parity shard (the last one).
        let (composite, backends) = make_erasure_composite(3, 2);

        upload_file(&composite, "/file1.enc", b"parity rebuild test").await;

        let target = 4; // Last parity shard.
        let shard_path =
            CompositeStorageProvider::shard_path(&VaultPath::parse("/file1.enc").unwrap(), target)
                .unwrap();

        backends[target].delete(&shard_path).await.unwrap();

        let rebuilder = RaidRebuilder::new(&composite, target, RebuildConfig::default()).unwrap();
        let result = rebuilder.rebuild().await.unwrap();

        assert_eq!(result.rebuilt, 1);
        assert!(backends[target].exists(&shard_path).await.unwrap());

        // Full data still downloadable.
        let vp = VaultPath::parse("/file1.enc").unwrap();
        let downloaded = composite.download(&vp).await.unwrap();
        assert_eq!(downloaded, b"parity rebuild test");
    }

    #[tokio::test]
    async fn test_erasure_rebuild_noop_empty_map() {
        let (composite, _backends) = make_erasure_composite(3, 2);

        let rebuilder = RaidRebuilder::new(&composite, 0, RebuildConfig::default()).unwrap();
        let result = rebuilder.rebuild().await.unwrap();

        assert_eq!(result.rebuilt, 0);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.failed, 0);
    }

    // -- Progress tracking tests ---------------------------------------------

    #[tokio::test]
    async fn test_progress_percentage() {
        let mut progress = RebuildProgress::new(10);
        assert_eq!(progress.percentage(), 0.0);

        progress.completed = 3;
        progress.skipped = 2;
        assert_eq!(progress.percentage(), 50.0);

        progress.failed = 5;
        assert_eq!(progress.percentage(), 100.0);
    }

    #[tokio::test]
    async fn test_progress_remaining() {
        let mut progress = RebuildProgress::new(10);
        assert_eq!(progress.remaining(), 10);

        progress.completed = 3;
        progress.skipped = 2;
        assert_eq!(progress.remaining(), 5);
    }

    #[tokio::test]
    async fn test_progress_empty_total() {
        let progress = RebuildProgress::new(0);
        assert_eq!(progress.percentage(), 100.0);
        assert_eq!(progress.remaining(), 0);
        assert!(progress.eta().is_none());
    }

    // -- Validation tests ----------------------------------------------------

    #[tokio::test]
    async fn test_invalid_target_index() {
        let (composite, _backends) = make_mirror_composite(3);
        let result = RaidRebuilder::new(&composite, 5, RebuildConfig::default());
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_zero_concurrency_rejected() {
        let (composite, _backends) = make_mirror_composite(3);
        let result = RaidRebuilder::new(
            &composite,
            0,
            RebuildConfig {
                concurrency: 0,
                ..Default::default()
            },
        );
        assert!(result.is_err());
    }

    // -- Idempotency test ----------------------------------------------------

    #[tokio::test]
    async fn test_mirror_rebuild_idempotent() {
        let (composite, backends) = make_mirror_composite(3);

        upload_file(&composite, "/file1.enc", b"idempotent").await;

        let vp = VaultPath::parse("/file1.enc").unwrap();
        backends[2].delete(&vp).await.unwrap();

        // First rebuild.
        let rebuilder = RaidRebuilder::new(&composite, 2, RebuildConfig::default()).unwrap();
        let r1 = rebuilder.rebuild().await.unwrap();
        assert_eq!(r1.rebuilt, 1);

        // Second rebuild — should skip everything.
        let rebuilder = RaidRebuilder::new(&composite, 2, RebuildConfig::default()).unwrap();
        let r2 = rebuilder.rebuild().await.unwrap();
        assert_eq!(r2.rebuilt, 0);
        assert_eq!(r2.skipped, 1);
    }

    // -- Checkpoint tests -------------------------------------------------------

    #[tokio::test]
    async fn test_checkpoint_save_load_roundtrip() {
        let backend = MemoryProvider::new();

        // Create and save a checkpoint.
        let mut ckpt = RebuildCheckpoint::new(1, RaidMode::Mirror);
        ckpt.completed_paths.insert("/a.enc".to_string());
        ckpt.completed_paths.insert("/b.enc".to_string());
        ckpt.save(&backend).await.unwrap();

        // Load it back.
        let loaded = RebuildCheckpoint::load(&backend, 1, RaidMode::Mirror)
            .await
            .expect("checkpoint should load");
        assert_eq!(loaded.target_index, 1);
        assert_eq!(loaded.completed_paths.len(), 2);
        assert!(loaded.completed_paths.contains("/a.enc"));
        assert!(loaded.completed_paths.contains("/b.enc"));
        assert_eq!(loaded.mode, RaidMode::Mirror);
    }

    #[tokio::test]
    async fn test_checkpoint_mismatched_params_ignored() {
        let backend = MemoryProvider::new();

        // Save a checkpoint for target_index=1, Mirror mode.
        let mut ckpt = RebuildCheckpoint::new(1, RaidMode::Mirror);
        ckpt.completed_paths.insert("/a.enc".to_string());
        ckpt.save(&backend).await.unwrap();

        // Loading with different target_index should return None.
        let loaded = RebuildCheckpoint::load(&backend, 2, RaidMode::Mirror).await;
        assert!(loaded.is_none());

        // Loading with different mode should return None.
        let loaded = RebuildCheckpoint::load(
            &backend,
            1,
            RaidMode::Erasure {
                data_shards: 3,
                parity_shards: 2,
            },
        )
        .await;
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_checkpoint_resume_skips_completed() {
        let (composite, backends) = make_mirror_composite(3);

        upload_file(&composite, "/a.enc", b"aaa").await;
        upload_file(&composite, "/b.enc", b"bbb").await;

        let pa = VaultPath::parse("/a.enc").unwrap();
        let pb = VaultPath::parse("/b.enc").unwrap();

        // Delete both files from backend 2.
        backends[2].delete(&pa).await.unwrap();
        backends[2].delete(&pb).await.unwrap();

        // Manually save a checkpoint claiming /a.enc is already done.
        let mut ckpt = RebuildCheckpoint::new(2, RaidMode::Mirror);
        ckpt.completed_paths.insert("/a.enc".to_string());
        ckpt.save(backends[2].as_ref()).await.unwrap();

        // Rebuild: /a.enc should be skipped via checkpoint (even though it's
        // not on the target), /b.enc should be rebuilt.
        let rebuilder = RaidRebuilder::new(&composite, 2, RebuildConfig::default()).unwrap();
        let result = rebuilder.rebuild().await.unwrap();

        // /a.enc was skipped (checkpoint), /b.enc was rebuilt.
        assert_eq!(result.rebuilt, 1);
        assert_eq!(result.skipped, 1);
        assert_eq!(result.failed, 0);

        // /b.enc is restored.
        assert_eq!(backends[2].download(&pb).await.unwrap(), b"bbb");

        // Checkpoint should be deleted after successful completion.
        let ckpt_path = VaultPath::parse(CHECKPOINT_PATH).unwrap();
        assert!(!backends[2].exists(&ckpt_path).await.unwrap());
    }

    #[tokio::test]
    async fn test_checkpoint_deleted_on_success() {
        let (composite, backends) = make_mirror_composite(3);

        upload_file(&composite, "/file1.enc", b"data").await;
        let vp = VaultPath::parse("/file1.enc").unwrap();
        backends[2].delete(&vp).await.unwrap();

        let config = RebuildConfig {
            concurrency: 1,
            checkpoint_interval: 1, // Save checkpoint after every chunk.
        };
        let rebuilder = RaidRebuilder::new(&composite, 2, config).unwrap();
        rebuilder.rebuild().await.unwrap();

        // Checkpoint should have been deleted.
        let ckpt_path = VaultPath::parse(CHECKPOINT_PATH).unwrap();
        assert!(!backends[2].exists(&ckpt_path).await.unwrap());
    }

    // -- Progress channel tests -------------------------------------------------

    #[tokio::test]
    async fn test_progress_channel_receives_updates() {
        let (composite, backends) = make_mirror_composite(3);

        upload_file(&composite, "/a.enc", b"aaa").await;
        upload_file(&composite, "/b.enc", b"bbb").await;

        // Delete both from backend 2.
        let pa = VaultPath::parse("/a.enc").unwrap();
        let pb = VaultPath::parse("/b.enc").unwrap();
        backends[2].delete(&pa).await.unwrap();
        backends[2].delete(&pb).await.unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel::<RebuildProgress>(32);
        let rebuilder = RaidRebuilder::new(&composite, 2, RebuildConfig::default())
            .unwrap()
            .with_progress_channel(tx);
        let result = rebuilder.rebuild().await.unwrap();

        assert_eq!(result.rebuilt, 2);
        assert_eq!(result.failed, 0);

        // We should have received at least one progress update per rebuilt chunk.
        let mut updates = Vec::new();
        while let Ok(p) = rx.try_recv() {
            updates.push(p);
        }
        assert!(
            updates.len() >= 2,
            "expected at least 2 progress updates, got {}",
            updates.len()
        );

        // The last update should show all chunks processed.
        let last = updates.last().unwrap();
        assert_eq!(last.completed + last.skipped + last.failed, last.total);
    }

    #[tokio::test]
    async fn test_progress_channel_optional() {
        // Verify rebuild works fine without a channel.
        let (composite, backends) = make_mirror_composite(3);
        upload_file(&composite, "/file1.enc", b"test").await;
        let vp = VaultPath::parse("/file1.enc").unwrap();
        backends[2].delete(&vp).await.unwrap();

        let rebuilder = RaidRebuilder::new(&composite, 2, RebuildConfig::default()).unwrap();
        let result = rebuilder.rebuild().await.unwrap();
        assert_eq!(result.rebuilt, 1);
    }

    // -- Backend-replace integration test ---------------------------------------

    #[tokio::test]
    async fn test_backend_replace_integration() {
        // 1. Create a composite with 3 MemoryProvider backends.
        let backends: Vec<Arc<dyn StorageProvider>> = (0..3)
            .map(|_| Arc::new(MemoryProvider::new()) as _)
            .collect();
        let config = CompositeConfig {
            mode: RaidMode::Mirror,
            health: Default::default(),
        };
        let composite =
            CompositeStorageProvider::new(backends.clone(), config.clone()).expect("composite");

        // 2. Upload several chunks.
        upload_file(&composite, "/doc1.enc", b"document one").await;
        upload_file(&composite, "/doc2.enc", b"document two").await;
        upload_file(&composite, "/doc3.enc", b"document three").await;

        // Verify all backends have all files.
        for path in &["/doc1.enc", "/doc2.enc", "/doc3.enc"] {
            let vp = VaultPath::parse(path).unwrap();
            for b in &backends {
                assert!(b.exists(&vp).await.unwrap(), "missing {} on backend", path);
            }
        }

        // 3. Replace backend 1 with a new empty MemoryProvider.
        let new_backend: Arc<dyn StorageProvider> = Arc::new(MemoryProvider::new());
        let mut new_backends = backends.clone();
        new_backends[1] = new_backend.clone();
        let composite2 =
            CompositeStorageProvider::new(new_backends.clone(), config).expect("composite2");

        // Manually load the shard map from the surviving backends.
        composite2.load_shard_map().await.unwrap();

        // Verify the new backend is empty.
        for path in &["/doc1.enc", "/doc2.enc", "/doc3.enc"] {
            let vp = VaultPath::parse(path).unwrap();
            assert!(!new_backend.exists(&vp).await.unwrap());
        }

        // 4. Run rebuild targeting the replaced backend.
        let rebuilder = RaidRebuilder::new(&composite2, 1, RebuildConfig::default()).unwrap();
        let result = rebuilder.rebuild().await.unwrap();

        assert_eq!(result.rebuilt, 3);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.failed, 0);

        // 5. Verify all original data is intact and accessible.
        let expected: &[(&str, &[u8])] = &[
            ("/doc1.enc", b"document one"),
            ("/doc2.enc", b"document two"),
            ("/doc3.enc", b"document three"),
        ];
        for &(path, data) in expected {
            let vp = VaultPath::parse(path).unwrap();
            let restored = new_backend.download(&vp).await.unwrap();
            assert_eq!(restored, data, "data mismatch for {}", path);
        }

        // Also verify the composite can download everything.
        for &(path, data) in expected {
            let vp = VaultPath::parse(path).unwrap();
            let downloaded = composite2.download(&vp).await.unwrap();
            assert_eq!(downloaded, data);
        }
    }
}
