//! Composite storage provider for multi-backend RAID operations.
//!
//! Wraps N `StorageProvider` backends behind the `StorageProvider` trait,
//! delegating operations according to the configured RAID mode.

mod config;
mod erasure;
mod fanout;
mod health;

pub use config::{CompositeConfig, RaidMode};

#[cfg(test)]
mod tests;

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream;
use reed_solomon_erasure::galois_8::ReedSolomon;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{OnceCell, RwLock};
use tracing::warn;

use crate::health::ProviderHealth;
use crate::provider::{ByteStream, Metadata, StorageProvider};
use crate::shard_map::ShardMap;
use axiomvault_common::{Error, Result, VaultPath};

/// A storage provider that distributes operations across multiple backends.
///
/// In mirror mode, writes fan out to all backends and reads return the first
/// successful result. In erasure mode, chunks are sharded via Reed-Solomon
/// coding across backends.
pub struct CompositeStorageProvider {
    pub(crate) backends: Vec<Arc<dyn StorageProvider>>,
    pub(crate) config: CompositeConfig,
    /// Cached Reed-Solomon encoder, created once for erasure mode.
    pub(crate) reed_solomon: Option<ReedSolomon>,
    /// Persistent shard map tracking chunk-to-backend mappings.
    pub(crate) shard_map: Arc<RwLock<ShardMap>>,
    /// One-shot cell that ensures shard map hydration runs exactly once.
    shard_map_init: OnceCell<()>,
    /// Per-backend health state for tracking availability and latency.
    pub(crate) health_states: Vec<Arc<RwLock<ProviderHealth>>>,
}

impl CompositeStorageProvider {
    /// Create a new composite provider.
    ///
    /// # Errors
    /// Returns `InvalidInput` if fewer than 2 backends are provided, or if
    /// erasure mode parameters don't match the backend count.
    pub fn new(backends: Vec<Arc<dyn StorageProvider>>, config: CompositeConfig) -> Result<Self> {
        if backends.len() < 2 {
            return Err(Error::InvalidInput(
                "CompositeStorageProvider requires at least 2 backends".to_string(),
            ));
        }

        config.health.validate().map_err(Error::InvalidInput)?;

        if let RaidMode::Erasure {
            data_shards,
            parity_shards,
        } = config.mode
        {
            if data_shards == 0 {
                return Err(Error::InvalidInput(
                    "Erasure mode requires at least 1 data shard".to_string(),
                ));
            }
            if parity_shards == 0 {
                return Err(Error::InvalidInput(
                    "Erasure mode requires at least 1 parity shard".to_string(),
                ));
            }
            if data_shards + parity_shards != backends.len() {
                return Err(Error::InvalidInput(format!(
                    "Erasure mode requires data_shards({}) + parity_shards({}) == backend count({})",
                    data_shards,
                    parity_shards,
                    backends.len()
                )));
            }
        }

        let reed_solomon = if let RaidMode::Erasure {
            data_shards,
            parity_shards,
        } = config.mode
        {
            Some(
                ReedSolomon::new(data_shards, parity_shards)
                    .map_err(|e| Error::Storage(format!("Reed-Solomon init failed: {}", e)))?,
            )
        } else {
            None
        };

        let health_states = (0..backends.len())
            .map(|i| Arc::new(RwLock::new(ProviderHealth::new(i))))
            .collect();

        Ok(Self {
            backends,
            config,
            reed_solomon,
            shard_map: Arc::new(RwLock::new(ShardMap::new())),
            shard_map_init: OnceCell::const_new(),
            health_states,
        })
    }

    /// Load the shard map from all backends, merging any diverged copies.
    ///
    /// Call this after construction to hydrate the shard map from persistent
    /// storage. If no backends have a shard map yet, this is a no-op.
    pub async fn load_shard_map(&self) -> Result<()> {
        let loaded = ShardMap::load_from_all(&self.backends).await?;
        *self.shard_map.write().await = loaded;
        let _ = self.shard_map_init.set(());
        Ok(())
    }

    /// Ensure the shard map has been hydrated from persistent storage.
    /// Called lazily before the first save to prevent overwriting existing data.
    ///
    /// Uses a `OnceCell` so at most one load ever runs, even under concurrency.
    /// If the load fails, the error is propagated and the `OnceCell` remains
    /// unset, allowing retry on the next call.
    async fn ensure_shard_map_loaded(&self) -> Result<()> {
        self.shard_map_init
            .get_or_try_init(|| async {
                let loaded = ShardMap::load_from_all(&self.backends).await?;
                *self.shard_map.write().await = loaded;
                Ok::<(), Error>(())
            })
            .await?;
        Ok(())
    }

    /// Persist the current shard map to all backends.
    pub async fn save_shard_map(&self) -> Result<()> {
        let map = self.shard_map.read().await;
        map.save_to_all(&self.backends).await
    }

    /// Get a clone of the current shard map (for inspection/testing).
    pub async fn get_shard_map(&self) -> ShardMap {
        self.shard_map.read().await.clone()
    }

    /// Get the current RAID mode.
    pub fn mode(&self) -> RaidMode {
        self.config.mode
    }

    /// Get a reference to the backend list.
    pub fn backends(&self) -> &[Arc<dyn StorageProvider>] {
        &self.backends
    }

    /// Get a reference to the composite configuration.
    pub fn composite_config(&self) -> &CompositeConfig {
        &self.config
    }

    /// Get a reference to the shard map (behind `Arc<RwLock<_>>`).
    pub fn shard_map_ref(&self) -> &Arc<RwLock<ShardMap>> {
        &self.shard_map
    }

    /// Get the number of backends.
    pub fn backend_count(&self) -> usize {
        self.backends.len()
    }

    /// Get the names of all backends.
    pub fn backend_names(&self) -> Vec<&str> {
        self.backends.iter().map(|b| b.name()).collect()
    }
}

/// Check whether a name matches the internal `.shardN` pattern.
fn is_shard_file(name: &str) -> bool {
    if let Some(pos) = name.rfind(".shard") {
        let suffix = &name[pos + 6..];
        !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit())
    } else {
        false
    }
}

#[async_trait]
impl StorageProvider for CompositeStorageProvider {
    fn name(&self) -> &str {
        "composite"
    }

    async fn upload(&self, path: &VaultPath, data: Vec<u8>) -> Result<Metadata> {
        self.ensure_shard_map_loaded().await?;
        match self.config.mode {
            RaidMode::Mirror => {
                let original_size = data.len() as u64;
                let path = path.clone();
                let data: Bytes = data.into();
                let (meta, succeeded) = self
                    .fan_out("upload", |backend| {
                        let path = path.clone();
                        let data = data.clone();
                        async move { backend.upload(&path, data.to_vec()).await }
                    })
                    .await?;

                // Record mirror entry with only the backends that succeeded
                let path_str = path.to_string_path();
                let entry = ShardMap::mirror_entry(
                    &path_str,
                    original_size,
                    &self.backends,
                    Some(&succeeded),
                );
                {
                    let mut map = self.shard_map.write().await;
                    map.insert(&path_str, entry);
                }
                self.save_shard_map().await?;

                Ok(meta)
            }
            RaidMode::Erasure { .. } => self.erasure_upload(path, data).await,
        }
    }

    async fn upload_stream(&self, path: &VaultPath, stream: ByteStream) -> Result<Metadata> {
        use futures::StreamExt;
        let mut data = Vec::new();
        let mut stream = stream;
        while let Some(chunk) = stream.next().await {
            data.extend_from_slice(&chunk?);
        }
        self.upload(path, data).await
    }

    async fn download(&self, path: &VaultPath) -> Result<Vec<u8>> {
        match self.config.mode {
            RaidMode::Mirror => {
                let path = path.clone();
                self.try_first("download", |backend| {
                    let path = path.clone();
                    async move { backend.download(&path).await }
                })
                .await
            }
            RaidMode::Erasure { .. } => self.erasure_download(path).await,
        }
    }

    async fn download_stream(&self, path: &VaultPath) -> Result<ByteStream> {
        let data = self.download(path).await?;
        Ok(Box::pin(stream::once(async move { Ok(data) })))
    }

    async fn exists(&self, path: &VaultPath) -> Result<bool> {
        match self.config.mode {
            RaidMode::Mirror => {
                let path = path.clone();
                self.try_first("exists", |backend| {
                    let path = path.clone();
                    async move { backend.exists(&path).await }
                })
                .await
            }
            RaidMode::Erasure { .. } => {
                // Try shard 0 across backends in order until one responds
                let shard_path = Self::shard_path(path, 0)?;
                self.try_first("exists", |backend| {
                    let sp = shard_path.clone();
                    async move { backend.exists(&sp).await }
                })
                .await
            }
        }
    }

    async fn delete(&self, path: &VaultPath) -> Result<()> {
        self.ensure_shard_map_loaded().await?;
        match self.config.mode {
            RaidMode::Mirror => {
                let path = path.clone();
                self.fan_out_void("delete", |backend| {
                    let path = path.clone();
                    async move { backend.delete(&path).await }
                })
                .await?;

                // Remove from shard map
                {
                    self.shard_map.write().await.remove(&path.to_string_path());
                }
                self.save_shard_map().await?;

                Ok(())
            }
            RaidMode::Erasure { .. } => self.erasure_delete(path).await,
        }
    }

    async fn list(&self, path: &VaultPath) -> Result<Vec<Metadata>> {
        let path = path.clone();

        // Query all backends concurrently and merge results.
        let futures: Vec<_> = self
            .backends
            .iter()
            .map(|b| {
                let p = path.clone();
                let backend = Arc::clone(b);
                async move { backend.list(&p).await }
            })
            .collect();
        let results = futures::future::join_all(futures).await;

        let mut merged: HashMap<String, Metadata> = HashMap::new();
        let mut any_success = false;
        let mut last_error: Option<Error> = None;

        for (i, result) in results.into_iter().enumerate() {
            match result {
                Ok(entries) => {
                    any_success = true;
                    for entry in entries {
                        merged
                            .entry(entry.name.clone())
                            .and_modify(|existing| {
                                if entry.modified > existing.modified {
                                    *existing = entry.clone();
                                }
                            })
                            .or_insert(entry);
                    }
                }
                Err(e) => {
                    warn!(
                        backend = self.backends[i].name(),
                        operation = "list",
                        error = %e,
                        "Backend list failed, continuing with other backends"
                    );
                    last_error = Some(e);
                }
            }
        }

        if any_success {
            let mut entries: Vec<Metadata> = merged.into_values().collect();
            // In erasure mode, filter out internal shard files
            if matches!(self.config.mode, RaidMode::Erasure { .. }) {
                entries.retain(|e| !is_shard_file(&e.name));
            }
            entries.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(entries)
        } else {
            Err(last_error
                .unwrap_or_else(|| Error::Storage("All backends failed for list".to_string())))
        }
    }

    async fn metadata(&self, path: &VaultPath) -> Result<Metadata> {
        match self.config.mode {
            RaidMode::Mirror => {
                let path = path.clone();
                self.try_first("metadata", |backend| {
                    let path = path.clone();
                    async move { backend.metadata(&path).await }
                })
                .await
            }
            RaidMode::Erasure { .. } => {
                let shard_path = Self::shard_path(path, 0)?;
                let file_name = path.name().unwrap_or("/").to_string();
                // Download shard 0 to read the original size from the header
                let shard_data = self
                    .try_first("metadata_download", |backend| {
                        let sp = shard_path.clone();
                        async move { backend.download(&sp).await }
                    })
                    .await?;
                // Header: [4-byte CRC-32] [8-byte LE original_size] [shard_data]
                if shard_data.len() < 12 {
                    return Err(Error::Storage(
                        "Shard too short to read metadata".to_string(),
                    ));
                }
                let original_size = u64::from_le_bytes(shard_data[4..12].try_into().unwrap());
                // Get filesystem metadata (timestamps etc) from the backend
                let shard_meta = self
                    .try_first("metadata", |backend| {
                        let sp = shard_path.clone();
                        async move { backend.metadata(&sp).await }
                    })
                    .await?;
                Ok(Metadata {
                    name: file_name,
                    size: Some(original_size),
                    is_directory: false,
                    ..shard_meta
                })
            }
        }
    }

    async fn create_dir(&self, path: &VaultPath) -> Result<Metadata> {
        let path = path.clone();
        let (meta, _) = self
            .fan_out("create_dir", |backend| {
                let path = path.clone();
                async move { backend.create_dir(&path).await }
            })
            .await?;
        Ok(meta)
    }

    async fn delete_dir(&self, path: &VaultPath) -> Result<()> {
        let path = path.clone();
        self.fan_out_void("delete_dir", |backend| {
            let path = path.clone();
            async move { backend.delete_dir(&path).await }
        })
        .await
    }

    async fn rename(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        self.ensure_shard_map_loaded().await?;
        match self.config.mode {
            RaidMode::Mirror => {
                let from = from.clone();
                let to = to.clone();
                let (meta, succeeded) = self
                    .fan_out("rename", |backend| {
                        let from = from.clone();
                        let to = to.clone();
                        async move { backend.rename(&from, &to).await }
                    })
                    .await?;

                // Update shard map: rename entry, then remove shards for failed backends
                let from_str = from.to_string_path();
                let to_str = to.to_string_path();
                {
                    let mut map = self.shard_map.write().await;
                    map.rename(&from_str, &to_str);
                    if let Some(entry) = map.entries.get_mut(&to_str) {
                        entry.shards.retain(|idx, _| succeeded.contains(idx));
                    }
                }
                self.save_shard_map().await?;

                Ok(meta)
            }
            RaidMode::Erasure { .. } => {
                let total = self.backends.len();
                let mut futures = Vec::with_capacity(total);
                for i in 0..total {
                    let backend = Arc::clone(&self.backends[i]);
                    let from_shard = Self::shard_path(from, i)?;
                    let to_shard = Self::shard_path(to, i)?;
                    futures.push(async move { backend.rename(&from_shard, &to_shard).await });
                }
                let meta = self
                    .erasure_per_shard("rename", to.name().unwrap_or("/").to_string(), futures)
                    .await?;

                // Update shard map
                let from_str = from.to_string_path();
                let to_str = to.to_string_path();
                {
                    self.shard_map.write().await.rename(&from_str, &to_str);
                }
                self.save_shard_map().await?;

                Ok(meta)
            }
        }
    }

    async fn copy(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        self.ensure_shard_map_loaded().await?;
        match self.config.mode {
            RaidMode::Mirror => {
                let from = from.clone();
                let to = to.clone();
                let (meta, succeeded) = self
                    .fan_out("copy", |backend| {
                        let from = from.clone();
                        let to = to.clone();
                        async move { backend.copy(&from, &to).await }
                    })
                    .await?;

                // Copy shard map entry for the new path, retaining only succeeded backends
                let from_str = from.to_string_path();
                let to_str = to.to_string_path();
                {
                    let mut map = self.shard_map.write().await;
                    if let Some(entry) = map.get(&from_str).cloned() {
                        let mut new_entry = entry;
                        for shard in new_entry.shards.values_mut() {
                            shard.backend_path = to_str.clone();
                        }
                        new_entry.shards.retain(|idx, _| succeeded.contains(idx));
                        new_entry.updated_at = chrono::Utc::now();
                        map.insert(&to_str, new_entry);
                    }
                }
                self.save_shard_map().await?;

                Ok(meta)
            }
            RaidMode::Erasure { .. } => {
                let total = self.backends.len();
                let mut futures = Vec::with_capacity(total);
                for i in 0..total {
                    let backend = Arc::clone(&self.backends[i]);
                    let from_shard = Self::shard_path(from, i)?;
                    let to_shard = Self::shard_path(to, i)?;
                    futures.push(async move { backend.copy(&from_shard, &to_shard).await });
                }
                let meta = self
                    .erasure_per_shard("copy", to.name().unwrap_or("/").to_string(), futures)
                    .await?;

                // Copy shard map entry for the new path
                let from_str = from.to_string_path();
                let to_str = to.to_string_path();
                {
                    let mut map = self.shard_map.write().await;
                    if let Some(entry) = map.get(&from_str).cloned() {
                        let mut new_entry = entry;
                        for shard in new_entry.shards.values_mut() {
                            if let Some(suffix) = shard.backend_path.strip_prefix(&from_str) {
                                shard.backend_path = format!("{}{}", to_str, suffix);
                            }
                        }
                        new_entry.updated_at = chrono::Utc::now();
                        map.insert(&to_str, new_entry);
                    }
                }
                self.save_shard_map().await?;

                Ok(meta)
            }
        }
    }
}
