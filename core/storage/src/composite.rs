//! Composite storage provider for multi-backend RAID operations.
//!
//! Wraps N `StorageProvider` backends behind the `StorageProvider` trait,
//! delegating operations according to the configured RAID mode.

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream;
use reed_solomon_erasure::galois_8::ReedSolomon;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::Arc;
use tracing::warn;

use std::collections::HashMap;

use crate::provider::{ByteStream, Metadata, StorageProvider};
use axiomvault_common::{Error, Result, VaultPath};

/// RAID mode for the composite provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RaidMode {
    /// Mirror (RAID 1): write all chunks to all backends, read from first success.
    Mirror,
    /// Erasure coding (RAID 5/6): Reed-Solomon sharding across backends.
    Erasure {
        data_shards: usize,
        parity_shards: usize,
    },
}

/// Configuration for the composite storage provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompositeConfig {
    /// RAID mode to use.
    pub mode: RaidMode,
}

/// A storage provider that distributes operations across multiple backends.
///
/// In mirror mode, writes fan out to all backends and reads return the first
/// successful result. In erasure mode, chunks are sharded via Reed-Solomon
/// coding across backends.
pub struct CompositeStorageProvider {
    backends: Vec<Arc<dyn StorageProvider>>,
    config: CompositeConfig,
    /// Cached Reed-Solomon encoder, created once for erasure mode.
    reed_solomon: Option<ReedSolomon>,
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

        Ok(Self {
            backends,
            config,
            reed_solomon,
        })
    }

    /// Get the current RAID mode.
    pub fn mode(&self) -> RaidMode {
        self.config.mode
    }

    /// Get the number of backends.
    pub fn backend_count(&self) -> usize {
        self.backends.len()
    }

    /// Get the names of all backends.
    pub fn backend_names(&self) -> Vec<&str> {
        self.backends.iter().map(|b| b.name()).collect()
    }

    /// Fan out a `Result<Metadata>` operation to all backends concurrently.
    /// Returns the first successful result; fails only if ALL backends fail.
    async fn fan_out<F, Fut>(&self, op: &str, f: F) -> Result<Metadata>
    where
        F: Fn(Arc<dyn StorageProvider>) -> Fut,
        Fut: Future<Output = Result<Metadata>>,
    {
        let futures: Vec<_> = self.backends.iter().map(|b| f(Arc::clone(b))).collect();
        let results = futures::future::join_all(futures).await;

        let mut first_success: Option<Metadata> = None;
        let mut last_error: Option<Error> = None;
        let mut failure_count = 0usize;

        for (i, result) in results.into_iter().enumerate() {
            match result {
                Ok(meta) => {
                    if first_success.is_none() {
                        first_success = Some(meta);
                    }
                }
                Err(e) => {
                    failure_count += 1;
                    warn!(
                        backend = self.backends[i].name(),
                        operation = op,
                        error = %e,
                        "Backend write failed"
                    );
                    last_error = Some(e);
                }
            }
        }

        if failure_count > 0 && first_success.is_some() {
            warn!(
                operation = op,
                failed = failure_count,
                total = self.backends.len(),
                "Partial write: {}/{} backends failed",
                failure_count,
                self.backends.len()
            );
        }

        first_success.ok_or_else(|| {
            last_error.unwrap_or_else(|| Error::Storage(format!("All backends failed for {}", op)))
        })
    }

    /// Fan out a `Result<()>` operation to all backends concurrently.
    async fn fan_out_void<F, Fut>(&self, op: &str, f: F) -> Result<()>
    where
        F: Fn(Arc<dyn StorageProvider>) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let futures: Vec<_> = self.backends.iter().map(|b| f(Arc::clone(b))).collect();
        let results = futures::future::join_all(futures).await;

        let mut any_success = false;
        let mut last_error: Option<Error> = None;
        let mut failure_count = 0usize;

        for (i, result) in results.into_iter().enumerate() {
            match result {
                Ok(()) => any_success = true,
                Err(e) => {
                    failure_count += 1;
                    warn!(
                        backend = self.backends[i].name(),
                        operation = op,
                        error = %e,
                        "Backend write failed"
                    );
                    last_error = Some(e);
                }
            }
        }

        if failure_count > 0 && any_success {
            warn!(
                operation = op,
                failed = failure_count,
                total = self.backends.len(),
                "Partial write: {}/{} backends failed",
                failure_count,
                self.backends.len()
            );
        }

        if any_success {
            Ok(())
        } else {
            Err(last_error
                .unwrap_or_else(|| Error::Storage(format!("All backends failed for {}", op))))
        }
    }

    /// Try backends in priority order, returning the first success.
    /// Only warns on errors that are not `NotFound` (which is a normal result).
    async fn try_first<T, F, Fut>(&self, op: &str, f: F) -> Result<T>
    where
        F: Fn(Arc<dyn StorageProvider>) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let mut last_error: Option<Error> = None;

        for backend in &self.backends {
            match f(Arc::clone(backend)).await {
                Ok(val) => return Ok(val),
                Err(e) => {
                    if !matches!(&e, Error::NotFound(_)) {
                        warn!(
                            backend = backend.name(),
                            operation = op,
                            error = %e,
                            "Backend read failed, trying next"
                        );
                    }
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| Error::Storage(format!("All backends failed for {}", op))))
    }

    /// Get erasure mode parameters, or error if in mirror mode.
    fn erasure_params(&self) -> Result<(usize, usize)> {
        match self.config.mode {
            RaidMode::Erasure {
                data_shards,
                parity_shards,
            } => Ok((data_shards, parity_shards)),
            RaidMode::Mirror => Err(Error::Storage("Not in erasure mode".to_string())),
        }
    }

    /// Get the cached Reed-Solomon encoder (only valid in erasure mode).
    fn reed_solomon(&self) -> &ReedSolomon {
        self.reed_solomon
            .as_ref()
            .expect("reed_solomon() called in mirror mode")
    }

    /// Encode data into N shards (k data + m parity) using Reed-Solomon.
    fn erasure_encode(&self, data: &[u8]) -> Result<Vec<Vec<u8>>> {
        let (data_shards, parity_shards) = self.erasure_params()?;
        let total_shards = data_shards + parity_shards;

        // Reed-Solomon requires equal-length shards; use ceil division for shard size.
        // Empty data gets minimum 1-byte shards since RS cannot operate on zero-length shards.
        let shard_size = if data.is_empty() {
            1
        } else {
            data.len().div_ceil(data_shards)
        };

        let mut shards: Vec<Vec<u8>> = Vec::with_capacity(total_shards);
        for i in 0..data_shards {
            let start = i * shard_size;
            let end = std::cmp::min(start + shard_size, data.len());
            let mut shard = if start < data.len() {
                data[start..end].to_vec()
            } else {
                Vec::new()
            };
            shard.resize(shard_size, 0);
            shards.push(shard);
        }
        for _ in 0..parity_shards {
            shards.push(vec![0u8; shard_size]);
        }

        self.reed_solomon()
            .encode(&mut shards)
            .map_err(|e| Error::Storage(format!("Reed-Solomon encode failed: {}", e)))?;

        Ok(shards)
    }

    /// Decode data from shards using Reed-Solomon.
    /// `shard_opts` has N entries; missing shards are `None`.
    fn erasure_decode(
        &self,
        mut shard_opts: Vec<Option<Vec<u8>>>,
        original_size: usize,
    ) -> Result<Vec<u8>> {
        let (data_shards, _) = self.erasure_params()?;

        self.reed_solomon()
            .reconstruct(&mut shard_opts)
            .map_err(|e| Error::Storage(format!("Reed-Solomon reconstruct failed: {}", e)))?;

        let mut data = Vec::with_capacity(original_size);
        for s in shard_opts.iter().take(data_shards).flatten() {
            data.extend_from_slice(s);
        }
        data.truncate(original_size);
        Ok(data)
    }

    /// Run a `Metadata`-returning operation per shard (one future per backend),
    /// collect results, and return the first success with the given `name`.
    async fn erasure_per_shard(
        &self,
        op: &str,
        name: String,
        futures: Vec<impl Future<Output = Result<Metadata>>>,
    ) -> Result<Metadata> {
        let results = futures::future::join_all(futures).await;
        let mut first_meta: Option<Metadata> = None;
        let mut last_error: Option<Error> = None;
        for (i, result) in results.into_iter().enumerate() {
            match result {
                Ok(meta) => {
                    if first_meta.is_none() {
                        first_meta = Some(Metadata {
                            name: name.clone(),
                            ..meta
                        });
                    }
                }
                Err(e) => {
                    warn!(backend = self.backends[i].name(), shard = i, error = %e, operation = op, "Erasure shard operation failed");
                    last_error = Some(e);
                }
            }
        }
        first_meta.ok_or_else(|| {
            last_error.unwrap_or_else(|| {
                Error::Storage(format!("All backends failed for erasure {}", op))
            })
        })
    }

    /// Build the shard path for a given file path and shard index.
    fn shard_path(path: &VaultPath, shard_index: usize) -> Result<VaultPath> {
        let path_str = path.to_string_path();
        VaultPath::parse(&format!("{}.shard{}", path_str, shard_index))
    }

    /// Upload data in erasure mode: encode into shards, write each to its backend.
    async fn erasure_upload(&self, path: &VaultPath, data: Vec<u8>) -> Result<Metadata> {
        let original_size = data.len();
        let shards = self.erasure_encode(&data)?;

        let mut futures = Vec::with_capacity(shards.len());
        for (i, shard) in shards.into_iter().enumerate() {
            let backend = Arc::clone(&self.backends[i]);
            let shard_path = Self::shard_path(path, i)?;
            // 8-byte LE size header so we can strip padding on reconstruct
            let mut payload = Vec::with_capacity(8 + shard.len());
            payload.extend_from_slice(&(original_size as u64).to_le_bytes());
            payload.extend_from_slice(&shard);
            futures.push(async move { backend.upload(&shard_path, payload).await });
        }

        let results = futures::future::join_all(futures).await;

        let mut first_meta: Option<Metadata> = None;
        let mut failure_count = 0usize;
        let mut last_error: Option<Error> = None;

        for (i, result) in results.into_iter().enumerate() {
            match result {
                Ok(meta) => {
                    if first_meta.is_none() {
                        // Return metadata reflecting the original file, not the shard
                        first_meta = Some(Metadata {
                            name: path.name().unwrap_or("/").to_string(),
                            size: Some(original_size as u64),
                            ..meta
                        });
                    }
                }
                Err(e) => {
                    failure_count += 1;
                    warn!(
                        backend = self.backends[i].name(),
                        shard = i,
                        error = %e,
                        "Erasure upload: shard write failed"
                    );
                    last_error = Some(e);
                }
            }
        }

        let (_, parity_shards) = self.erasure_params()?;
        if failure_count > parity_shards {
            return Err(last_error
                .unwrap_or_else(|| Error::Storage("Too many shard write failures".to_string())));
        }

        if failure_count > 0 {
            warn!(
                operation = "erasure_upload",
                failed = failure_count,
                total = self.backends.len(),
                "Partial erasure write: {}/{} shards failed",
                failure_count,
                self.backends.len()
            );
        }

        first_meta.ok_or_else(|| {
            last_error
                .unwrap_or_else(|| Error::Storage("All backends failed for erasure upload".into()))
        })
    }

    /// Download data in erasure mode: fetch shards, reconstruct original data.
    async fn erasure_download(&self, path: &VaultPath) -> Result<Vec<u8>> {
        let RaidMode::Erasure {
            data_shards,
            parity_shards,
        } = self.config.mode
        else {
            return Err(Error::Storage("Not in erasure mode".to_string()));
        };

        let total = data_shards + parity_shards;
        let mut futures = Vec::with_capacity(total);
        for i in 0..total {
            let backend = Arc::clone(&self.backends[i]);
            let shard_path = Self::shard_path(path, i)?;
            futures.push(async move { (i, backend.download(&shard_path).await) });
        }

        let results = futures::future::join_all(futures).await;

        let mut shard_opts: Vec<Option<Vec<u8>>> = vec![None; total];
        let mut original_size: Option<usize> = None;
        let mut available = 0usize;

        for (i, result) in results {
            if let Ok(payload) = result {
                if payload.len() < 8 {
                    warn!(shard = i, "Erasure download: shard too short, skipping");
                    continue;
                }
                let size = u64::from_le_bytes(payload[..8].try_into().unwrap()) as usize;
                if original_size.is_none() {
                    original_size = Some(size);
                }
                shard_opts[i] = Some(payload[8..].to_vec());
                available += 1;
            }
        }

        if available < data_shards {
            return Err(Error::Storage(format!(
                "Erasure download: only {}/{} shards available, need {}",
                available, total, data_shards
            )));
        }

        let original_size = original_size
            .ok_or_else(|| Error::Storage("Erasure download: no shards available".to_string()))?;

        self.erasure_decode(shard_opts, original_size)
    }

    /// Delete shards across all backends in erasure mode.
    async fn erasure_delete(&self, path: &VaultPath) -> Result<()> {
        let total = self.backends.len();
        let mut futures = Vec::with_capacity(total);
        for i in 0..total {
            let backend = Arc::clone(&self.backends[i]);
            let shard_path = Self::shard_path(path, i)?;
            futures.push(async move { backend.delete(&shard_path).await });
        }

        let results = futures::future::join_all(futures).await;
        let mut any_success = false;
        let mut last_error: Option<Error> = None;

        for (i, result) in results.into_iter().enumerate() {
            match result {
                Ok(()) => any_success = true,
                Err(e) => {
                    warn!(
                        backend = self.backends[i].name(),
                        shard = i,
                        error = %e,
                        "Erasure delete: shard delete failed"
                    );
                    last_error = Some(e);
                }
            }
        }

        if any_success {
            Ok(())
        } else {
            Err(last_error
                .unwrap_or_else(|| Error::Storage("All backends failed for erasure delete".into())))
        }
    }
}

#[async_trait]
impl StorageProvider for CompositeStorageProvider {
    fn name(&self) -> &str {
        "composite"
    }

    async fn upload(&self, path: &VaultPath, data: Vec<u8>) -> Result<Metadata> {
        match self.config.mode {
            RaidMode::Mirror => {
                let path = path.clone();
                let data: Bytes = data.into();
                self.fan_out("upload", |backend| {
                    let path = path.clone();
                    let data = data.clone();
                    async move { backend.upload(&path, data.to_vec()).await }
                })
                .await
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
        match self.config.mode {
            RaidMode::Mirror => {
                let path = path.clone();
                self.fan_out_void("delete", |backend| {
                    let path = path.clone();
                    async move { backend.delete(&path).await }
                })
                .await
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
                // Try shard 0 across backends for resilience; return metadata
                // reflecting the logical file rather than the shard
                let shard_path = Self::shard_path(path, 0)?;
                let file_name = path.name().unwrap_or("/").to_string();
                let shard_meta = self
                    .try_first("metadata", |backend| {
                        let sp = shard_path.clone();
                        async move { backend.metadata(&sp).await }
                    })
                    .await?;
                Ok(Metadata {
                    name: file_name,
                    is_directory: false,
                    ..shard_meta
                })
            }
        }
    }

    async fn create_dir(&self, path: &VaultPath) -> Result<Metadata> {
        let path = path.clone();
        self.fan_out("create_dir", |backend| {
            let path = path.clone();
            async move { backend.create_dir(&path).await }
        })
        .await
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
        match self.config.mode {
            RaidMode::Mirror => {
                let from = from.clone();
                let to = to.clone();
                self.fan_out("rename", |backend| {
                    let from = from.clone();
                    let to = to.clone();
                    async move { backend.rename(&from, &to).await }
                })
                .await
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
                self.erasure_per_shard("rename", to.name().unwrap_or("/").to_string(), futures)
                    .await
            }
        }
    }

    async fn copy(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        match self.config.mode {
            RaidMode::Mirror => {
                let from = from.clone();
                let to = to.clone();
                self.fan_out("copy", |backend| {
                    let from = from.clone();
                    let to = to.clone();
                    async move { backend.copy(&from, &to).await }
                })
                .await
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
                self.erasure_per_shard("copy", to.name().unwrap_or("/").to_string(), futures)
                    .await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryProvider;

    fn make_backends(n: usize) -> Vec<Arc<dyn StorageProvider>> {
        (0..n)
            .map(|_| Arc::new(MemoryProvider::new()) as Arc<dyn StorageProvider>)
            .collect()
    }

    fn mirror_config() -> CompositeConfig {
        CompositeConfig {
            mode: RaidMode::Mirror,
        }
    }

    // -- A provider that always fails, for partial-failure tests -----------

    struct FailingProvider;

    #[async_trait]
    impl StorageProvider for FailingProvider {
        fn name(&self) -> &str {
            "failing"
        }
        async fn upload(&self, _: &VaultPath, _: Vec<u8>) -> Result<Metadata> {
            Err(Error::Storage("backend down".into()))
        }
        async fn upload_stream(&self, _: &VaultPath, _: ByteStream) -> Result<Metadata> {
            Err(Error::Storage("backend down".into()))
        }
        async fn download(&self, _: &VaultPath) -> Result<Vec<u8>> {
            Err(Error::Storage("backend down".into()))
        }
        async fn download_stream(&self, _: &VaultPath) -> Result<ByteStream> {
            Err(Error::Storage("backend down".into()))
        }
        async fn exists(&self, _: &VaultPath) -> Result<bool> {
            Err(Error::Storage("backend down".into()))
        }
        async fn delete(&self, _: &VaultPath) -> Result<()> {
            Err(Error::Storage("backend down".into()))
        }
        async fn list(&self, _: &VaultPath) -> Result<Vec<Metadata>> {
            Err(Error::Storage("backend down".into()))
        }
        async fn metadata(&self, _: &VaultPath) -> Result<Metadata> {
            Err(Error::Storage("backend down".into()))
        }
        async fn create_dir(&self, _: &VaultPath) -> Result<Metadata> {
            Err(Error::Storage("backend down".into()))
        }
        async fn delete_dir(&self, _: &VaultPath) -> Result<()> {
            Err(Error::Storage("backend down".into()))
        }
        async fn rename(&self, _: &VaultPath, _: &VaultPath) -> Result<Metadata> {
            Err(Error::Storage("backend down".into()))
        }
        async fn copy(&self, _: &VaultPath, _: &VaultPath) -> Result<Metadata> {
            Err(Error::Storage("backend down".into()))
        }
    }

    // -- Construction tests ------------------------------------------------

    #[test]
    fn test_requires_minimum_two_backends() {
        let result = CompositeStorageProvider::new(make_backends(1), mirror_config());
        assert!(result.is_err());
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("at least 2 backends"));
    }

    #[test]
    fn test_construction_with_two_backends() {
        let provider = CompositeStorageProvider::new(make_backends(2), mirror_config()).unwrap();
        assert_eq!(provider.backend_count(), 2);
        assert_eq!(provider.name(), "composite");
        assert_eq!(provider.mode(), RaidMode::Mirror);
    }

    #[test]
    fn test_erasure_validation() {
        // Mismatched shard count
        assert!(CompositeStorageProvider::new(
            make_backends(3),
            CompositeConfig {
                mode: RaidMode::Erasure {
                    data_shards: 2,
                    parity_shards: 2,
                },
            },
        )
        .is_err());

        // Valid erasure config
        assert!(CompositeStorageProvider::new(
            make_backends(5),
            CompositeConfig {
                mode: RaidMode::Erasure {
                    data_shards: 3,
                    parity_shards: 2,
                },
            },
        )
        .is_ok());

        // Zero data shards
        assert!(CompositeStorageProvider::new(
            make_backends(2),
            CompositeConfig {
                mode: RaidMode::Erasure {
                    data_shards: 0,
                    parity_shards: 2,
                },
            },
        )
        .is_err());
    }

    #[test]
    fn test_backend_names() {
        let provider = CompositeStorageProvider::new(make_backends(3), mirror_config()).unwrap();
        let names = provider.backend_names();
        assert_eq!(names.len(), 3);
        assert!(names.iter().all(|n| *n == "memory"));
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = CompositeConfig {
            mode: RaidMode::Erasure {
                data_shards: 3,
                parity_shards: 2,
            },
        };
        let json = serde_json::to_string(&config).unwrap();
        let decoded: CompositeConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.mode, config.mode);
    }

    // -- Mirror happy-path tests -------------------------------------------

    #[tokio::test]
    async fn test_mirror_upload_download() {
        let backends = make_backends(3);
        let provider = CompositeStorageProvider::new(backends.clone(), mirror_config()).unwrap();

        let path = VaultPath::parse("/test.txt").unwrap();
        let data = b"hello world".to_vec();

        provider.upload(&path, data.clone()).await.unwrap();

        // All backends should have the data
        for backend in &backends {
            assert_eq!(backend.download(&path).await.unwrap(), data);
        }

        // Download via composite should work
        assert_eq!(provider.download(&path).await.unwrap(), data);
    }

    #[tokio::test]
    async fn test_mirror_exists() {
        let provider = CompositeStorageProvider::new(make_backends(2), mirror_config()).unwrap();

        let path = VaultPath::parse("/test.txt").unwrap();
        assert!(!provider.exists(&path).await.unwrap());

        provider.upload(&path, vec![1, 2, 3]).await.unwrap();
        assert!(provider.exists(&path).await.unwrap());
    }

    #[tokio::test]
    async fn test_mirror_delete() {
        let backends = make_backends(3);
        let provider = CompositeStorageProvider::new(backends.clone(), mirror_config()).unwrap();

        let path = VaultPath::parse("/test.txt").unwrap();
        provider.upload(&path, vec![1, 2, 3]).await.unwrap();
        provider.delete(&path).await.unwrap();

        for backend in &backends {
            assert!(!backend.exists(&path).await.unwrap());
        }
    }

    #[tokio::test]
    async fn test_mirror_create_dir_and_list() {
        let provider = CompositeStorageProvider::new(make_backends(2), mirror_config()).unwrap();

        let dir = VaultPath::parse("/mydir").unwrap();
        let meta = provider.create_dir(&dir).await.unwrap();
        assert!(meta.is_directory);

        provider
            .upload(&VaultPath::parse("/mydir/file.txt").unwrap(), vec![42])
            .await
            .unwrap();

        let listing = provider.list(&dir).await.unwrap();
        assert_eq!(listing.len(), 1);
        assert_eq!(listing[0].name, "file.txt");
    }

    #[tokio::test]
    async fn test_mirror_rename() {
        let provider = CompositeStorageProvider::new(make_backends(2), mirror_config()).unwrap();

        let from = VaultPath::parse("/old.txt").unwrap();
        let to = VaultPath::parse("/new.txt").unwrap();

        provider.upload(&from, vec![1, 2, 3]).await.unwrap();
        provider.rename(&from, &to).await.unwrap();

        assert!(!provider.exists(&from).await.unwrap());
        assert!(provider.exists(&to).await.unwrap());
    }

    #[tokio::test]
    async fn test_mirror_copy() {
        let provider = CompositeStorageProvider::new(make_backends(2), mirror_config()).unwrap();

        let from = VaultPath::parse("/original.txt").unwrap();
        let to = VaultPath::parse("/copy.txt").unwrap();
        let data = vec![1, 2, 3];

        provider.upload(&from, data.clone()).await.unwrap();
        provider.copy(&from, &to).await.unwrap();

        assert!(provider.exists(&from).await.unwrap());
        assert_eq!(provider.download(&to).await.unwrap(), data);
    }

    #[tokio::test]
    async fn test_mirror_metadata() {
        let provider = CompositeStorageProvider::new(make_backends(2), mirror_config()).unwrap();

        let path = VaultPath::parse("/test.txt").unwrap();
        provider.upload(&path, vec![1, 2, 3]).await.unwrap();

        let meta = provider.metadata(&path).await.unwrap();
        assert_eq!(meta.name, "test.txt");
        assert_eq!(meta.size, Some(3));
        assert!(!meta.is_directory);
    }

    #[tokio::test]
    async fn test_mirror_delete_dir() {
        let provider = CompositeStorageProvider::new(make_backends(2), mirror_config()).unwrap();

        let dir = VaultPath::parse("/mydir").unwrap();
        provider.create_dir(&dir).await.unwrap();
        provider.delete_dir(&dir).await.unwrap();

        assert!(!provider.exists(&dir).await.unwrap());
    }

    #[tokio::test]
    async fn test_mirror_upload_stream() {
        let provider = CompositeStorageProvider::new(make_backends(2), mirror_config()).unwrap();

        let path = VaultPath::parse("/stream.txt").unwrap();
        let data = vec![10, 20, 30];
        let data_clone = data.clone();
        let stream: ByteStream = Box::pin(futures::stream::once(async move { Ok(data_clone) }));

        provider.upload_stream(&path, stream).await.unwrap();
        assert_eq!(provider.download(&path).await.unwrap(), data);
    }

    #[tokio::test]
    async fn test_mirror_download_stream() {
        let provider = CompositeStorageProvider::new(make_backends(2), mirror_config()).unwrap();

        let path = VaultPath::parse("/stream.txt").unwrap();
        let data = vec![10, 20, 30];
        provider.upload(&path, data.clone()).await.unwrap();

        use futures::StreamExt;
        let mut stream = provider.download_stream(&path).await.unwrap();
        let mut result = Vec::new();
        while let Some(chunk) = stream.next().await {
            result.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(result, data);
    }

    // -- Partial-failure tests ---------------------------------------------

    #[tokio::test]
    async fn test_mirror_upload_succeeds_with_one_failing_backend() {
        let backends: Vec<Arc<dyn StorageProvider>> =
            vec![Arc::new(FailingProvider), Arc::new(MemoryProvider::new())];
        let provider = CompositeStorageProvider::new(backends, mirror_config()).unwrap();

        let path = VaultPath::parse("/test.txt").unwrap();
        // Should succeed — the MemoryProvider backend is healthy
        provider.upload(&path, vec![1, 2, 3]).await.unwrap();
        assert_eq!(provider.download(&path).await.unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn test_mirror_upload_fails_when_all_backends_fail() {
        let backends: Vec<Arc<dyn StorageProvider>> =
            vec![Arc::new(FailingProvider), Arc::new(FailingProvider)];
        let provider = CompositeStorageProvider::new(backends, mirror_config()).unwrap();

        let path = VaultPath::parse("/test.txt").unwrap();
        assert!(provider.upload(&path, vec![1, 2, 3]).await.is_err());
    }

    #[tokio::test]
    async fn test_mirror_download_falls_back_to_healthy_backend() {
        let healthy = Arc::new(MemoryProvider::new());
        let path = VaultPath::parse("/test.txt").unwrap();
        healthy.upload(&path, vec![42]).await.unwrap();

        let backends: Vec<Arc<dyn StorageProvider>> = vec![Arc::new(FailingProvider), healthy];
        let provider = CompositeStorageProvider::new(backends, mirror_config()).unwrap();

        // First backend fails, second succeeds
        assert_eq!(provider.download(&path).await.unwrap(), vec![42]);
    }

    #[tokio::test]
    async fn test_mirror_delete_succeeds_with_partial_failure() {
        let healthy = Arc::new(MemoryProvider::new());
        let path = VaultPath::parse("/test.txt").unwrap();
        healthy.upload(&path, vec![1]).await.unwrap();

        let backends: Vec<Arc<dyn StorageProvider>> =
            vec![Arc::new(FailingProvider), healthy.clone()];
        let provider = CompositeStorageProvider::new(backends, mirror_config()).unwrap();

        provider.delete(&path).await.unwrap();
        assert!(!healthy.exists(&path).await.unwrap());
    }

    // -- Mirror list merge tests ----------------------------------------------

    #[tokio::test]
    async fn test_mirror_list_returns_union_of_diverged_backends() {
        let backend_a = Arc::new(MemoryProvider::new());
        let backend_b = Arc::new(MemoryProvider::new());

        let dir = VaultPath::parse("/data").unwrap();
        backend_a.create_dir(&dir).await.unwrap();
        backend_b.create_dir(&dir).await.unwrap();

        // backend_a has file1 only
        let file1 = VaultPath::parse("/data/file1.txt").unwrap();
        backend_a.upload(&file1, vec![1]).await.unwrap();

        // backend_b has file1 and file2
        backend_b.upload(&file1, vec![1]).await.unwrap();
        let file2 = VaultPath::parse("/data/file2.txt").unwrap();
        backend_b.upload(&file2, vec![2]).await.unwrap();

        let backends: Vec<Arc<dyn StorageProvider>> = vec![backend_a, backend_b];
        let provider = CompositeStorageProvider::new(backends, mirror_config()).unwrap();

        let listing = provider.list(&dir).await.unwrap();
        let names: Vec<&str> = listing.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["file1.txt", "file2.txt"]);
    }

    #[tokio::test]
    async fn test_mirror_list_deduplicates_by_name_keeping_newest() {
        let backend_a = Arc::new(MemoryProvider::new());
        let backend_b = Arc::new(MemoryProvider::new());

        let dir = VaultPath::parse("/data").unwrap();
        backend_a.create_dir(&dir).await.unwrap();
        backend_b.create_dir(&dir).await.unwrap();

        // Upload to backend_a first (older timestamp)
        let file = VaultPath::parse("/data/file.txt").unwrap();
        backend_a.upload(&file, vec![1]).await.unwrap();
        let meta_a = backend_a.metadata(&file).await.unwrap();

        // Small delay to ensure different timestamps
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Upload to backend_b second (newer timestamp)
        backend_b.upload(&file, vec![1, 2]).await.unwrap();
        let meta_b = backend_b.metadata(&file).await.unwrap();

        // Verify backend_b's timestamp is newer
        assert!(meta_b.modified >= meta_a.modified);

        let backends: Vec<Arc<dyn StorageProvider>> = vec![backend_a, backend_b];
        let provider = CompositeStorageProvider::new(backends, mirror_config()).unwrap();

        let listing = provider.list(&dir).await.unwrap();
        assert_eq!(listing.len(), 1);
        assert_eq!(listing[0].name, "file.txt");
        // Should keep backend_b's entry (newer), which has size 2
        assert_eq!(listing[0].size, Some(2));
    }

    #[tokio::test]
    async fn test_mirror_list_succeeds_with_one_failing_backend() {
        let healthy = Arc::new(MemoryProvider::new());
        let dir = VaultPath::parse("/data").unwrap();
        healthy.create_dir(&dir).await.unwrap();
        let file = VaultPath::parse("/data/file.txt").unwrap();
        healthy.upload(&file, vec![1]).await.unwrap();

        let backends: Vec<Arc<dyn StorageProvider>> = vec![Arc::new(FailingProvider), healthy];
        let provider = CompositeStorageProvider::new(backends, mirror_config()).unwrap();

        let listing = provider.list(&dir).await.unwrap();
        assert_eq!(listing.len(), 1);
        assert_eq!(listing[0].name, "file.txt");
    }

    // -- Erasure coding tests -------------------------------------------------

    fn erasure_config(data_shards: usize, parity_shards: usize) -> CompositeConfig {
        CompositeConfig {
            mode: RaidMode::Erasure {
                data_shards,
                parity_shards,
            },
        }
    }

    #[tokio::test]
    async fn test_erasure_upload_download_basic() {
        let backends = make_backends(5);
        let provider =
            CompositeStorageProvider::new(backends.clone(), erasure_config(3, 2)).unwrap();

        let path = VaultPath::parse("/test.txt").unwrap();
        let data = b"hello world erasure coding test data".to_vec();

        let meta = provider.upload(&path, data.clone()).await.unwrap();
        assert_eq!(meta.name, "test.txt");
        assert_eq!(meta.size, Some(data.len() as u64));

        let downloaded = provider.download(&path).await.unwrap();
        assert_eq!(downloaded, data);
    }

    #[tokio::test]
    async fn test_erasure_upload_download_empty_data() {
        let backends = make_backends(3);
        let provider =
            CompositeStorageProvider::new(backends.clone(), erasure_config(2, 1)).unwrap();

        let path = VaultPath::parse("/empty.bin").unwrap();
        let data = vec![];

        provider.upload(&path, data.clone()).await.unwrap();
        let downloaded = provider.download(&path).await.unwrap();
        assert_eq!(downloaded, data);
    }

    #[tokio::test]
    async fn test_erasure_upload_download_single_byte() {
        let backends = make_backends(3);
        let provider =
            CompositeStorageProvider::new(backends.clone(), erasure_config(2, 1)).unwrap();

        let path = VaultPath::parse("/one.bin").unwrap();
        let data = vec![42];

        provider.upload(&path, data.clone()).await.unwrap();
        let downloaded = provider.download(&path).await.unwrap();
        assert_eq!(downloaded, data);
    }

    #[tokio::test]
    async fn test_erasure_upload_download_large_data() {
        let backends = make_backends(5);
        let provider =
            CompositeStorageProvider::new(backends.clone(), erasure_config(3, 2)).unwrap();

        let path = VaultPath::parse("/large.bin").unwrap();
        let data: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();

        provider.upload(&path, data.clone()).await.unwrap();
        let downloaded = provider.download(&path).await.unwrap();
        assert_eq!(downloaded, data);
    }

    #[tokio::test]
    async fn test_erasure_reconstruct_with_exactly_k_shards() {
        // 3 data + 2 parity = 5 backends. Remove 2 (parity count) shards.
        let backends = make_backends(5);
        let provider =
            CompositeStorageProvider::new(backends.clone(), erasure_config(3, 2)).unwrap();

        let path = VaultPath::parse("/recover.txt").unwrap();
        let data = b"data that must survive two backend failures".to_vec();

        provider.upload(&path, data.clone()).await.unwrap();

        // Delete shards from backends 0 and 1 (simulating 2 backend failures)
        let shard0 = VaultPath::parse("/recover.txt.shard0").unwrap();
        let shard1 = VaultPath::parse("/recover.txt.shard1").unwrap();
        backends[0].delete(&shard0).await.unwrap();
        backends[1].delete(&shard1).await.unwrap();

        // Should still reconstruct from remaining 3 shards
        let downloaded = provider.download(&path).await.unwrap();
        assert_eq!(downloaded, data);
    }

    #[tokio::test]
    async fn test_erasure_fails_with_too_few_shards() {
        // 3 data + 2 parity = 5 backends. Remove 3 shards (more than parity).
        let backends = make_backends(5);
        let provider =
            CompositeStorageProvider::new(backends.clone(), erasure_config(3, 2)).unwrap();

        let path = VaultPath::parse("/fail.txt").unwrap();
        let data = b"this will be unrecoverable".to_vec();

        provider.upload(&path, data).await.unwrap();

        // Delete 3 shards — only 2 remain, need 3
        backends[0]
            .delete(&VaultPath::parse("/fail.txt.shard0").unwrap())
            .await
            .unwrap();
        backends[1]
            .delete(&VaultPath::parse("/fail.txt.shard1").unwrap())
            .await
            .unwrap();
        backends[2]
            .delete(&VaultPath::parse("/fail.txt.shard2").unwrap())
            .await
            .unwrap();

        let result = provider.download(&path).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("shards available"));
    }

    #[tokio::test]
    async fn test_erasure_exists() {
        let backends = make_backends(3);
        let provider =
            CompositeStorageProvider::new(backends.clone(), erasure_config(2, 1)).unwrap();

        let path = VaultPath::parse("/check.bin").unwrap();
        assert!(!provider.exists(&path).await.unwrap());

        provider.upload(&path, vec![1, 2, 3]).await.unwrap();
        assert!(provider.exists(&path).await.unwrap());
    }

    #[tokio::test]
    async fn test_erasure_delete() {
        let backends = make_backends(3);
        let provider =
            CompositeStorageProvider::new(backends.clone(), erasure_config(2, 1)).unwrap();

        let path = VaultPath::parse("/del.bin").unwrap();
        provider.upload(&path, vec![1, 2, 3]).await.unwrap();

        provider.delete(&path).await.unwrap();

        // All shard files should be gone
        for (i, backend) in backends.iter().enumerate() {
            let shard_path = VaultPath::parse(&format!("/del.bin.shard{}", i)).unwrap();
            assert!(!backend.exists(&shard_path).await.unwrap());
        }
    }

    #[tokio::test]
    async fn test_erasure_rename() {
        let backends = make_backends(3);
        let provider =
            CompositeStorageProvider::new(backends.clone(), erasure_config(2, 1)).unwrap();

        let from = VaultPath::parse("/old.bin").unwrap();
        let to = VaultPath::parse("/new.bin").unwrap();
        let data = vec![10, 20, 30];

        provider.upload(&from, data.clone()).await.unwrap();
        provider.rename(&from, &to).await.unwrap();

        assert!(!provider.exists(&from).await.unwrap());
        assert!(provider.exists(&to).await.unwrap());
        assert_eq!(provider.download(&to).await.unwrap(), data);
    }

    #[tokio::test]
    async fn test_erasure_copy() {
        let backends = make_backends(3);
        let provider =
            CompositeStorageProvider::new(backends.clone(), erasure_config(2, 1)).unwrap();

        let from = VaultPath::parse("/src.bin").unwrap();
        let to = VaultPath::parse("/dst.bin").unwrap();
        let data = vec![5, 10, 15, 20];

        provider.upload(&from, data.clone()).await.unwrap();
        provider.copy(&from, &to).await.unwrap();

        assert!(provider.exists(&from).await.unwrap());
        assert_eq!(provider.download(&from).await.unwrap(), data);
        assert_eq!(provider.download(&to).await.unwrap(), data);
    }

    #[tokio::test]
    async fn test_erasure_metadata() {
        let backends = make_backends(3);
        let provider =
            CompositeStorageProvider::new(backends.clone(), erasure_config(2, 1)).unwrap();

        let path = VaultPath::parse("/meta.bin").unwrap();
        provider.upload(&path, vec![1, 2, 3]).await.unwrap();

        let meta = provider.metadata(&path).await.unwrap();
        assert_eq!(meta.name, "meta.bin");
        assert!(!meta.is_directory);
    }

    #[tokio::test]
    async fn test_erasure_stream_upload_download() {
        let backends = make_backends(3);
        let provider =
            CompositeStorageProvider::new(backends.clone(), erasure_config(2, 1)).unwrap();

        let path = VaultPath::parse("/stream.bin").unwrap();
        let data = vec![100, 200, 255, 0, 1];
        let data_clone = data.clone();
        let stream: ByteStream = Box::pin(futures::stream::once(async move { Ok(data_clone) }));

        provider.upload_stream(&path, stream).await.unwrap();

        use futures::StreamExt;
        let mut download_stream = provider.download_stream(&path).await.unwrap();
        let mut result = Vec::new();
        while let Some(chunk) = download_stream.next().await {
            result.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(result, data);
    }

    #[tokio::test]
    async fn test_erasure_5_backends_3_of_5_knock_out_2() {
        // Integration test per issue #145: 5 MemoryProvider backends, 3-of-5, knock out 2
        let backends = make_backends(5);
        let provider =
            CompositeStorageProvider::new(backends.clone(), erasure_config(3, 2)).unwrap();

        // Upload multiple files
        let files: Vec<(VaultPath, Vec<u8>)> = (0..5)
            .map(|i| {
                let path = VaultPath::parse(&format!("/file{}.dat", i)).unwrap();
                let data: Vec<u8> = (0..=255).cycle().take(100 + i * 50).collect();
                (path, data)
            })
            .collect();

        for (path, data) in &files {
            provider.upload(path, data.clone()).await.unwrap();
        }

        // Knock out backends 3 and 4 by deleting their shards
        for (path, _) in &files {
            let path_str = path.to_string_path();
            for shard_idx in [3, 4] {
                let shard_path =
                    VaultPath::parse(&format!("{}.shard{}", path_str, shard_idx)).unwrap();
                backends[shard_idx].delete(&shard_path).await.unwrap();
            }
        }

        // All files should still be readable (3 of 5 shards available)
        for (path, expected_data) in &files {
            let downloaded = provider.download(path).await.unwrap();
            assert_eq!(&downloaded, expected_data, "Data mismatch for {}", path);
        }
    }

    #[tokio::test]
    async fn test_erasure_data_not_divisible_by_k() {
        // Data length not evenly divisible by data_shards — tests padding
        let backends = make_backends(4);
        let provider =
            CompositeStorageProvider::new(backends.clone(), erasure_config(3, 1)).unwrap();

        let path = VaultPath::parse("/odd.bin").unwrap();
        // 7 bytes / 3 shards = 2 remainder 1 → tests padding correctness
        let data = vec![1, 2, 3, 4, 5, 6, 7];

        provider.upload(&path, data.clone()).await.unwrap();
        let downloaded = provider.download(&path).await.unwrap();
        assert_eq!(downloaded, data);
    }
}
