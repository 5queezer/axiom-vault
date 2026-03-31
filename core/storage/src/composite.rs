//! Composite storage provider for multi-backend RAID operations.
//!
//! Wraps N `StorageProvider` backends behind the `StorageProvider` trait,
//! delegating operations according to the configured RAID mode.

use async_trait::async_trait;
use futures::stream;
use std::sync::Arc;
use tracing::warn;

use crate::provider::{ByteStream, Metadata, StorageProvider};
use axiomvault_common::{Error, Result, VaultPath};

/// RAID mode for the composite provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaidMode {
    /// Mirror (RAID 1): write all chunks to all backends, read from first success.
    Mirror,
    /// Erasure coding (RAID 5/6): Reed-Solomon sharding across backends.
    /// Fields: (data_shards, parity_shards).
    Erasure { data_shards: usize, parity_shards: usize },
}

/// Configuration for the composite storage provider.
#[derive(Debug, Clone)]
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

        if let RaidMode::Erasure { data_shards, parity_shards } = config.mode {
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
                    data_shards, parity_shards, backends.len()
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
}

/// Fan out a write operation to all backends, returning the first successful
/// metadata result. Logs warnings for individual backend failures.
/// Returns error only if ALL backends fail.
macro_rules! fan_out_write {
    ($self:expr, $op_name:expr, $call:expr) => {{
        let futures: Vec<_> = $self
            .backends
            .iter()
            .map(|backend| {
                let backend = Arc::clone(backend);
                $call(backend)
            })
            .collect();

        let results = futures::future::join_all(futures).await;

        let mut first_success: Option<Metadata> = None;
        let mut last_error: Option<Error> = None;
        let mut failure_count = 0;

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
                        backend = $self.backends[i].name(),
                        operation = $op_name,
                        error = %e,
                        "Backend write failed"
                    );
                    last_error = Some(e);
                }
            }
        }

        if failure_count > 0 && first_success.is_some() {
            warn!(
                operation = $op_name,
                failed = failure_count,
                total = $self.backends.len(),
                "Partial write failure: {}/{} backends failed",
                failure_count,
                $self.backends.len()
            );
        }

        match first_success {
            Some(meta) => Ok(meta),
            None => Err(last_error.unwrap_or_else(|| {
                Error::Storage(format!("All backends failed for {}", $op_name))
            })),
        }
    }};
}

/// Try backends in order for a read operation, returning the first success.
macro_rules! try_read {
    ($self:expr, $op_name:expr, $call:expr) => {{
        let mut last_error = None;

        for backend in &$self.backends {
            match $call(Arc::clone(backend)).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    warn!(
                        backend = backend.name(),
                        operation = $op_name,
                        error = %e,
                        "Backend read failed, trying next"
                    );
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            Error::Storage(format!("All backends failed for {}", $op_name))
        }))
    }};
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
                let data = Arc::new(data);
                fan_out_write!(self, "upload", |backend: Arc<dyn StorageProvider>| {
                    let path = path.clone();
                    let data = Arc::clone(&data);
                    async move { backend.upload(&path, (*data).clone()).await }
                })
            }
            RaidMode::Erasure { .. } => {
                Err(Error::Storage("Erasure mode not yet implemented".to_string()))
            }
        }
    }

    async fn upload_stream(&self, path: &VaultPath, stream: ByteStream) -> Result<Metadata> {
        // For streams, we must buffer the data to replicate across backends.
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
                try_read!(self, "download", |backend: Arc<dyn StorageProvider>| {
                    let path = path.clone();
                    async move { backend.download(&path).await }
                })
            }
            RaidMode::Erasure { .. } => {
                Err(Error::Storage("Erasure mode not yet implemented".to_string()))
            }
        }
    }

    async fn download_stream(&self, path: &VaultPath) -> Result<ByteStream> {
        // Download full data then wrap as a single-chunk stream.
        let data = self.download(path).await?;
        let stream = stream::once(async move { Ok(data) });
        Ok(Box::pin(stream))
    }

    async fn exists(&self, path: &VaultPath) -> Result<bool> {
        try_read!(self, "exists", |backend: Arc<dyn StorageProvider>| {
            let path = path.clone();
            async move { backend.exists(&path).await }
        })
    }

    async fn delete(&self, path: &VaultPath) -> Result<()> {
        match self.config.mode {
            RaidMode::Mirror => {
                let path = path.clone();
                let futures: Vec<_> = self
                    .backends
                    .iter()
                    .map(|backend| {
                        let backend = Arc::clone(backend);
                        let path = path.clone();
                        async move { backend.delete(&path).await }
                    })
                    .collect();

                let results = futures::future::join_all(futures).await;

                let mut any_success = false;
                let mut last_error = None;
                let mut failure_count = 0;

                for (i, result) in results.into_iter().enumerate() {
                    match result {
                        Ok(()) => any_success = true,
                        Err(e) => {
                            failure_count += 1;
                            warn!(
                                backend = self.backends[i].name(),
                                operation = "delete",
                                error = %e,
                                "Backend delete failed"
                            );
                            last_error = Some(e);
                        }
                    }
                }

                if failure_count > 0 && any_success {
                    warn!(
                        operation = "delete",
                        failed = failure_count,
                        total = self.backends.len(),
                        "Partial delete failure: {}/{} backends failed",
                        failure_count,
                        self.backends.len()
                    );
                }

                if any_success {
                    Ok(())
                } else {
                    Err(last_error.unwrap_or_else(|| {
                        Error::Storage("All backends failed for delete".to_string())
                    }))
                }
            }
            RaidMode::Erasure { .. } => {
                Err(Error::Storage("Erasure mode not yet implemented".to_string()))
            }
        }
    }

    async fn list(&self, path: &VaultPath) -> Result<Vec<Metadata>> {
        match self.config.mode {
            RaidMode::Mirror => {
                // In mirror mode, all backends should have the same contents.
                // Read from first successful backend.
                try_read!(self, "list", |backend: Arc<dyn StorageProvider>| {
                    let path = path.clone();
                    async move { backend.list(&path).await }
                })
            }
            RaidMode::Erasure { .. } => {
                Err(Error::Storage("Erasure mode not yet implemented".to_string()))
            }
        }
    }

    async fn metadata(&self, path: &VaultPath) -> Result<Metadata> {
        try_read!(self, "metadata", |backend: Arc<dyn StorageProvider>| {
            let path = path.clone();
            async move { backend.metadata(&path).await }
        })
    }

    async fn create_dir(&self, path: &VaultPath) -> Result<Metadata> {
        match self.config.mode {
            RaidMode::Mirror => {
                let path = path.clone();
                fan_out_write!(self, "create_dir", |backend: Arc<dyn StorageProvider>| {
                    let path = path.clone();
                    async move { backend.create_dir(&path).await }
                })
            }
            RaidMode::Erasure { .. } => {
                Err(Error::Storage("Erasure mode not yet implemented".to_string()))
            }
        }
    }

    async fn delete_dir(&self, path: &VaultPath) -> Result<()> {
        match self.config.mode {
            RaidMode::Mirror => {
                let path = path.clone();
                let futures: Vec<_> = self
                    .backends
                    .iter()
                    .map(|backend| {
                        let backend = Arc::clone(backend);
                        let path = path.clone();
                        async move { backend.delete_dir(&path).await }
                    })
                    .collect();

                let results = futures::future::join_all(futures).await;

                let mut any_success = false;
                let mut last_error = None;
                let mut failure_count = 0;

                for (i, result) in results.into_iter().enumerate() {
                    match result {
                        Ok(()) => any_success = true,
                        Err(e) => {
                            failure_count += 1;
                            warn!(
                                backend = self.backends[i].name(),
                                operation = "delete_dir",
                                error = %e,
                                "Backend delete_dir failed"
                            );
                            last_error = Some(e);
                        }
                    }
                }

                if failure_count > 0 && any_success {
                    warn!(
                        operation = "delete_dir",
                        failed = failure_count,
                        total = self.backends.len(),
                        "Partial delete_dir failure: {}/{} backends failed",
                        failure_count,
                        self.backends.len()
                    );
                }

                if any_success {
                    Ok(())
                } else {
                    Err(last_error.unwrap_or_else(|| {
                        Error::Storage("All backends failed for delete_dir".to_string())
                    }))
                }
            }
            RaidMode::Erasure { .. } => {
                Err(Error::Storage("Erasure mode not yet implemented".to_string()))
            }
        }
    }

    async fn rename(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        match self.config.mode {
            RaidMode::Mirror => {
                let from = from.clone();
                let to = to.clone();
                fan_out_write!(self, "rename", |backend: Arc<dyn StorageProvider>| {
                    let from = from.clone();
                    let to = to.clone();
                    async move { backend.rename(&from, &to).await }
                })
            }
            RaidMode::Erasure { .. } => {
                Err(Error::Storage("Erasure mode not yet implemented".to_string()))
            }
        }
    }

    async fn copy(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        match self.config.mode {
            RaidMode::Mirror => {
                let from = from.clone();
                let to = to.clone();
                fan_out_write!(self, "copy", |backend: Arc<dyn StorageProvider>| {
                    let from = from.clone();
                    let to = to.clone();
                    async move { backend.copy(&from, &to).await }
                })
            }
            RaidMode::Erasure { .. } => {
                Err(Error::Storage("Erasure mode not yet implemented".to_string()))
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

    #[test]
    fn test_requires_minimum_two_backends() {
        let result = CompositeStorageProvider::new(make_backends(1), mirror_config());
        assert!(result.is_err());
        let err = match result {
            Err(e) => e,
            Ok(_) => unreachable!(),
        };
        assert!(err.to_string().contains("at least 2 backends"));
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
        let result = CompositeStorageProvider::new(
            make_backends(3),
            CompositeConfig {
                mode: RaidMode::Erasure {
                    data_shards: 2,
                    parity_shards: 2,
                },
            },
        );
        assert!(result.is_err());

        // Valid erasure config
        let result = CompositeStorageProvider::new(
            make_backends(5),
            CompositeConfig {
                mode: RaidMode::Erasure {
                    data_shards: 3,
                    parity_shards: 2,
                },
            },
        );
        assert!(result.is_ok());

        // Zero data shards
        let result = CompositeStorageProvider::new(
            make_backends(2),
            CompositeConfig {
                mode: RaidMode::Erasure {
                    data_shards: 0,
                    parity_shards: 2,
                },
            },
        );
        assert!(result.is_err());
    }

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
            let downloaded = backend.download(&path).await.unwrap();
            assert_eq!(downloaded, data);
        }

        // Download via composite should work
        let downloaded = provider.download(&path).await.unwrap();
        assert_eq!(downloaded, data);
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

        // All backends should have the file deleted
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

    #[test]
    fn test_backend_names() {
        let provider =
            CompositeStorageProvider::new(make_backends(3), mirror_config()).unwrap();
        let names = provider.backend_names();
        assert_eq!(names.len(), 3);
        assert!(names.iter().all(|n| *n == "memory"));
    }
}
