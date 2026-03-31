//! Composite storage provider for multi-backend RAID operations.
//!
//! Wraps N `StorageProvider` backends behind the `StorageProvider` trait,
//! delegating operations according to the configured RAID mode.

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::Arc;
use tracing::warn;

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
/// successful result. In erasure mode (future), chunks are sharded via
/// Reed-Solomon coding.
pub struct CompositeStorageProvider {
    backends: Vec<Arc<dyn StorageProvider>>,
    config: CompositeConfig,
}

impl CompositeStorageProvider {
    /// Create a new composite provider.
    ///
    /// # Errors
    /// Returns `InvalidInput` if fewer than 2 backends are provided, or if
    /// erasure mode parameters don't match the backend count.
    pub fn new(
        backends: Vec<Arc<dyn StorageProvider>>,
        config: CompositeConfig,
    ) -> Result<Self> {
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

        Ok(Self { backends, config })
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
            last_error
                .unwrap_or_else(|| Error::Storage(format!("All backends failed for {}", op)))
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

        Err(last_error
            .unwrap_or_else(|| Error::Storage(format!("All backends failed for {}", op))))
    }

    fn require_mirror(&self, op: &str) -> Result<()> {
        match self.config.mode {
            RaidMode::Mirror => Ok(()),
            RaidMode::Erasure { .. } => Err(Error::Storage(format!(
                "Erasure mode not yet implemented for {}",
                op
            ))),
        }
    }
}

#[async_trait]
impl StorageProvider for CompositeStorageProvider {
    fn name(&self) -> &str {
        "composite"
    }

    async fn upload(&self, path: &VaultPath, data: Vec<u8>) -> Result<Metadata> {
        self.require_mirror("upload")?;
        let path = path.clone();
        let data: Bytes = data.into();
        self.fan_out("upload", |backend| {
            let path = path.clone();
            let data = data.clone();
            async move { backend.upload(&path, data.to_vec()).await }
        })
        .await
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
        self.require_mirror("download")?;
        let path = path.clone();
        self.try_first("download", |backend| {
            let path = path.clone();
            async move { backend.download(&path).await }
        })
        .await
    }

    async fn download_stream(&self, path: &VaultPath) -> Result<ByteStream> {
        let data = self.download(path).await?;
        Ok(Box::pin(stream::once(async move { Ok(data) })))
    }

    async fn exists(&self, path: &VaultPath) -> Result<bool> {
        let path = path.clone();
        self.try_first("exists", |backend| {
            let path = path.clone();
            async move { backend.exists(&path).await }
        })
        .await
    }

    async fn delete(&self, path: &VaultPath) -> Result<()> {
        self.require_mirror("delete")?;
        let path = path.clone();
        self.fan_out_void("delete", |backend| {
            let path = path.clone();
            async move { backend.delete(&path).await }
        })
        .await
    }

    async fn list(&self, path: &VaultPath) -> Result<Vec<Metadata>> {
        self.require_mirror("list")?;
        let path = path.clone();
        self.try_first("list", |backend| {
            let path = path.clone();
            async move { backend.list(&path).await }
        })
        .await
    }

    async fn metadata(&self, path: &VaultPath) -> Result<Metadata> {
        let path = path.clone();
        self.try_first("metadata", |backend| {
            let path = path.clone();
            async move { backend.metadata(&path).await }
        })
        .await
    }

    async fn create_dir(&self, path: &VaultPath) -> Result<Metadata> {
        self.require_mirror("create_dir")?;
        let path = path.clone();
        self.fan_out("create_dir", |backend| {
            let path = path.clone();
            async move { backend.create_dir(&path).await }
        })
        .await
    }

    async fn delete_dir(&self, path: &VaultPath) -> Result<()> {
        self.require_mirror("delete_dir")?;
        let path = path.clone();
        self.fan_out_void("delete_dir", |backend| {
            let path = path.clone();
            async move { backend.delete_dir(&path).await }
        })
        .await
    }

    async fn rename(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        self.require_mirror("rename")?;
        let from = from.clone();
        let to = to.clone();
        self.fan_out("rename", |backend| {
            let from = from.clone();
            let to = to.clone();
            async move { backend.rename(&from, &to).await }
        })
        .await
    }

    async fn copy(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        self.require_mirror("copy")?;
        let from = from.clone();
        let to = to.clone();
        self.fan_out("copy", |backend| {
            let from = from.clone();
            let to = to.clone();
            async move { backend.copy(&from, &to).await }
        })
        .await
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
        assert!(result.err().unwrap().to_string().contains("at least 2 backends"));
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
        let provider =
            CompositeStorageProvider::new(make_backends(3), mirror_config()).unwrap();
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
        let provider =
            CompositeStorageProvider::new(backends.clone(), mirror_config()).unwrap();

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
        let provider =
            CompositeStorageProvider::new(make_backends(2), mirror_config()).unwrap();

        let path = VaultPath::parse("/test.txt").unwrap();
        assert!(!provider.exists(&path).await.unwrap());

        provider.upload(&path, vec![1, 2, 3]).await.unwrap();
        assert!(provider.exists(&path).await.unwrap());
    }

    #[tokio::test]
    async fn test_mirror_delete() {
        let backends = make_backends(3);
        let provider =
            CompositeStorageProvider::new(backends.clone(), mirror_config()).unwrap();

        let path = VaultPath::parse("/test.txt").unwrap();
        provider.upload(&path, vec![1, 2, 3]).await.unwrap();
        provider.delete(&path).await.unwrap();

        for backend in &backends {
            assert!(!backend.exists(&path).await.unwrap());
        }
    }

    #[tokio::test]
    async fn test_mirror_create_dir_and_list() {
        let provider =
            CompositeStorageProvider::new(make_backends(2), mirror_config()).unwrap();

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
        let provider =
            CompositeStorageProvider::new(make_backends(2), mirror_config()).unwrap();

        let from = VaultPath::parse("/old.txt").unwrap();
        let to = VaultPath::parse("/new.txt").unwrap();

        provider.upload(&from, vec![1, 2, 3]).await.unwrap();
        provider.rename(&from, &to).await.unwrap();

        assert!(!provider.exists(&from).await.unwrap());
        assert!(provider.exists(&to).await.unwrap());
    }

    #[tokio::test]
    async fn test_mirror_copy() {
        let provider =
            CompositeStorageProvider::new(make_backends(2), mirror_config()).unwrap();

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
        let provider =
            CompositeStorageProvider::new(make_backends(2), mirror_config()).unwrap();

        let path = VaultPath::parse("/test.txt").unwrap();
        provider.upload(&path, vec![1, 2, 3]).await.unwrap();

        let meta = provider.metadata(&path).await.unwrap();
        assert_eq!(meta.name, "test.txt");
        assert_eq!(meta.size, Some(3));
        assert!(!meta.is_directory);
    }

    #[tokio::test]
    async fn test_mirror_delete_dir() {
        let provider =
            CompositeStorageProvider::new(make_backends(2), mirror_config()).unwrap();

        let dir = VaultPath::parse("/mydir").unwrap();
        provider.create_dir(&dir).await.unwrap();
        provider.delete_dir(&dir).await.unwrap();

        assert!(!provider.exists(&dir).await.unwrap());
    }

    #[tokio::test]
    async fn test_mirror_upload_stream() {
        let provider =
            CompositeStorageProvider::new(make_backends(2), mirror_config()).unwrap();

        let path = VaultPath::parse("/stream.txt").unwrap();
        let data = vec![10, 20, 30];
        let data_clone = data.clone();
        let stream: ByteStream =
            Box::pin(futures::stream::once(async move { Ok(data_clone) }));

        provider.upload_stream(&path, stream).await.unwrap();
        assert_eq!(provider.download(&path).await.unwrap(), data);
    }

    #[tokio::test]
    async fn test_mirror_download_stream() {
        let provider =
            CompositeStorageProvider::new(make_backends(2), mirror_config()).unwrap();

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
        let backends: Vec<Arc<dyn StorageProvider>> = vec![
            Arc::new(FailingProvider),
            Arc::new(MemoryProvider::new()),
        ];
        let provider = CompositeStorageProvider::new(backends, mirror_config()).unwrap();

        let path = VaultPath::parse("/test.txt").unwrap();
        // Should succeed — the MemoryProvider backend is healthy
        provider.upload(&path, vec![1, 2, 3]).await.unwrap();
        assert_eq!(provider.download(&path).await.unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn test_mirror_upload_fails_when_all_backends_fail() {
        let backends: Vec<Arc<dyn StorageProvider>> = vec![
            Arc::new(FailingProvider),
            Arc::new(FailingProvider),
        ];
        let provider = CompositeStorageProvider::new(backends, mirror_config()).unwrap();

        let path = VaultPath::parse("/test.txt").unwrap();
        assert!(provider.upload(&path, vec![1, 2, 3]).await.is_err());
    }

    #[tokio::test]
    async fn test_mirror_download_falls_back_to_healthy_backend() {
        let healthy = Arc::new(MemoryProvider::new());
        let path = VaultPath::parse("/test.txt").unwrap();
        healthy.upload(&path, vec![42]).await.unwrap();

        let backends: Vec<Arc<dyn StorageProvider>> =
            vec![Arc::new(FailingProvider), healthy];
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
}
