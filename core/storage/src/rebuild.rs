//! Recovery and rebuild of missing shards.
//!
//! When a backend recovers after failure or is replaced, the `RaidRebuilder`
//! reconstructs missing data on the target backend. In mirror mode it copies
//! whole chunks from a healthy peer; in erasure mode it reconstructs the
//! specific missing shard from the remaining shards via Reed-Solomon.
//!
//! Rebuilds are incremental (existing data is skipped) and resumable
//! (re-running the rebuild picks up where it left off).

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::stream::{self, StreamExt};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::composite::{CompositeStorageProvider, RaidMode};
use crate::provider::StorageProvider;
use axiomvault_common::{Error, Result, VaultPath};

/// Configuration for a rebuild operation.
#[derive(Debug, Clone)]
pub struct RebuildConfig {
    /// Maximum number of concurrent transfers during rebuild. Default: 4.
    pub concurrency: usize,
}

impl Default for RebuildConfig {
    fn default() -> Self {
        Self { concurrency: 4 }
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
}

impl<'a> RaidRebuilder<'a> {
    /// Create a new rebuilder targeting the backend at `target_index`.
    pub fn new(
        composite: &'a CompositeStorageProvider,
        target_index: usize,
        config: RebuildConfig,
    ) -> Result<Self> {
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
        })
    }

    /// Get a snapshot of current rebuild progress.
    pub async fn progress(&self) -> RebuildProgress {
        self.progress.read().await.clone()
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

        // Build a list of (path, source_backend_index) for chunks that are missing on target.
        let mut work_items: Vec<(String, usize)> = Vec::new();
        let mut skipped = 0usize;

        for (path, _) in &entries {
            let vault_path = VaultPath::parse(path)?;
            match target.exists(&vault_path).await {
                Ok(true) => {
                    skipped += 1;
                }
                _ => {
                    // Find a healthy source backend that has this chunk.
                    let entry = shard_map.get(path);
                    let source_index = entry.and_then(|e| {
                        e.shards
                            .values()
                            .find(|s| s.shard_index != target_index)
                            .map(|s| s.shard_index)
                    });
                    if let Some(src) = source_index {
                        work_items.push((path.clone(), src));
                    } else {
                        // No source available — count as failed.
                        let mut p = progress.write().await;
                        p.failed += 1;
                        warn!(
                            path = path.as_str(),
                            "Mirror rebuild: no source backend available"
                        );
                    }
                }
            }
        }

        {
            let mut p = progress.write().await;
            p.skipped = skipped;
        }

        // Process work items with bounded concurrency.
        let composite = self.composite;
        let results: Vec<bool> = stream::iter(work_items)
            .map(|(path, source_index)| {
                let progress = Arc::clone(&progress);
                async move {
                    let result =
                        Self::copy_chunk(composite.backends(), source_index, target_index, &path)
                            .await;
                    let mut p = progress.write().await;
                    match result {
                        Ok(()) => {
                            p.completed += 1;
                            true
                        }
                        Err(e) => {
                            warn!(path = path.as_str(), error = %e, "Mirror rebuild: failed to copy chunk");
                            p.failed += 1;
                            false
                        }
                    }
                }
            })
            .buffer_unordered(self.config.concurrency)
            .collect()
            .await;

        let rebuilt = results.iter().filter(|&&ok| ok).count();
        let failed_copies = results.iter().filter(|&&ok| !ok).count();

        // Update shard map to include target backend in rebuilt entries.
        if rebuilt > 0 {
            let mut map = self.composite.shard_map_ref().write().await;
            // Collect paths that need updating (async exists check cannot run during iter_mut).
            let paths_to_check: Vec<String> = map
                .entries
                .iter()
                .filter(|(_, e)| !e.shards.contains_key(&target_index))
                .map(|(p, _)| p.clone())
                .collect();
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
                    if let Some(entry) = map.entries.get_mut(&path) {
                        let backend_id =
                            format!("{}:{}", backends[target_index].name(), target_index);
                        entry.shards.insert(
                            target_index,
                            crate::shard_map::ShardLocation {
                                shard_index: target_index,
                                backend_id,
                                backend_path: path.clone(),
                                is_parity: false,
                            },
                        );
                        entry.updated_at = chrono::Utc::now();
                    }
                }
            }
            map.version += 1;
            map.updated_at = chrono::Utc::now();
        }

        // Persist updated shard map.
        if rebuilt > 0 {
            self.composite.save_shard_map().await?;
        }

        let progress = self.progress.read().await;
        Ok(RebuildResult {
            rebuilt,
            skipped: progress.skipped,
            failed: progress.failed + failed_copies,
            elapsed: started_at.elapsed(),
        })
    }

    /// Copy a single chunk from source to target backend.
    async fn copy_chunk(
        backends: &[Arc<dyn StorageProvider>],
        source_index: usize,
        target_index: usize,
        path: &str,
    ) -> Result<()> {
        let vault_path = VaultPath::parse(path)?;
        let data = backends[source_index].download(&vault_path).await?;
        backends[target_index].upload(&vault_path, data).await?;
        Ok(())
    }

    /// Erasure-mode rebuild: reconstruct missing shards from available peers.
    async fn rebuild_erasure(&self) -> Result<RebuildResult> {
        let shard_map = self.composite.get_shard_map().await;
        let backends = self.composite.backends();
        let (data_shards, parity_shards) = self.composite.erasure_params()?;
        let total_shards = data_shards + parity_shards;

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

        // Determine which entries need rebuilding.
        let mut work_items: Vec<(String, crate::shard_map::ChunkEntry)> = Vec::new();
        let mut skipped = 0usize;

        for (path, entry) in &entries {
            let shard_path =
                CompositeStorageProvider::shard_path(&VaultPath::parse(path)?, target_index)?;
            match backends[target_index].exists(&shard_path).await {
                Ok(true) => {
                    skipped += 1;
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

        // Process work items with bounded concurrency.
        let composite = self.composite;
        let results: Vec<bool> = stream::iter(work_items)
            .map(|(path, _entry)| {
                let progress = Arc::clone(&progress);
                async move {
                    let result = Self::rebuild_erasure_shard(
                        composite,
                        target_index,
                        &path,
                        data_shards,
                        total_shards,
                    )
                    .await;
                    let mut p = progress.write().await;
                    match result {
                        Ok(()) => {
                            p.completed += 1;
                            true
                        }
                        Err(e) => {
                            warn!(path = path.as_str(), error = %e, "Erasure rebuild: failed to reconstruct shard");
                            p.failed += 1;
                            false
                        }
                    }
                }
            })
            .buffer_unordered(self.config.concurrency)
            .collect()
            .await;

        let rebuilt = results.iter().filter(|&&ok| ok).count();

        // Update shard map entries for rebuilt shards.
        if rebuilt > 0 {
            let mut map = self.composite.shard_map_ref().write().await;
            let paths_to_check: Vec<String> = map
                .entries
                .iter()
                .filter(|(_, e)| !e.shards.contains_key(&target_index))
                .map(|(p, _)| p.clone())
                .collect();
            for path in paths_to_check {
                let vault_path = match VaultPath::parse(&path) {
                    Ok(vp) => vp,
                    Err(_) => continue,
                };
                let shard_path =
                    match CompositeStorageProvider::shard_path(&vault_path, target_index) {
                        Ok(sp) => sp,
                        Err(_) => continue,
                    };
                if backends[target_index]
                    .exists(&shard_path)
                    .await
                    .unwrap_or(false)
                {
                    if let Some(entry) = map.entries.get_mut(&path) {
                        let backend_id =
                            format!("{}:{}", backends[target_index].name(), target_index);
                        entry.shards.insert(
                            target_index,
                            crate::shard_map::ShardLocation {
                                shard_index: target_index,
                                backend_id,
                                backend_path: format!("{}.shard{}", path, target_index),
                                is_parity: target_index >= data_shards,
                            },
                        );
                        entry.updated_at = chrono::Utc::now();
                    }
                }
            }
            map.version += 1;
            map.updated_at = chrono::Utc::now();
        }

        if rebuilt > 0 {
            self.composite.save_shard_map().await?;
        }

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
        data_shards: usize,
        total_shards: usize,
    ) -> Result<()> {
        let vault_path = VaultPath::parse(path)?;
        let backends = composite.backends();

        // Download all available shards from peer backends.
        let mut shard_opts: Vec<Option<Vec<u8>>> = vec![None; total_shards];
        let mut original_size: Option<u64> = None;
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
                    let size = u64::from_le_bytes(payload[4..12].try_into().unwrap());
                    let shard_data = &payload[12..];

                    // Verify CRC-32 integrity.
                    let computed_crc = crc32fast::hash(shard_data);
                    if stored_crc != computed_crc {
                        warn!(shard = i, path, "Rebuild: CRC mismatch, skipping shard");
                        continue;
                    }

                    if original_size.is_none() {
                        original_size = Some(size);
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

        let original_size = original_size
            .ok_or_else(|| Error::Storage(format!("Rebuild: no original_size for '{}'", path)))?;

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
}
