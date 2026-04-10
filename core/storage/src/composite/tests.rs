use super::*;
use crate::health::{HealthConfig, HealthStatus};
use crate::memory::MemoryProvider;
use crate::provider::ByteStream;
use crate::shard_map::ShardMap;
use async_trait::async_trait;
use axiomvault_common::{Error, Result, VaultPath};
use std::sync::Arc;

fn make_backends(n: usize) -> Vec<Arc<dyn StorageProvider>> {
    (0..n)
        .map(|_| Arc::new(MemoryProvider::new()) as Arc<dyn StorageProvider>)
        .collect()
}

fn mirror_config() -> CompositeConfig {
    CompositeConfig {
        mode: RaidMode::Mirror,
        health: HealthConfig::default(),
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
            health: HealthConfig::default(),
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
            health: HealthConfig::default(),
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
            health: HealthConfig::default(),
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
        health: HealthConfig::default(),
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

/// Seed an empty shard map on a backend so `load_from_all` finds `Ok(Some(_))`
/// even when other backends in the composite are failing.
async fn seed_empty_shard_map(backend: &dyn StorageProvider) {
    ShardMap::new().save_to_backend(backend).await.unwrap();
}

#[tokio::test]
async fn test_mirror_upload_succeeds_with_one_failing_backend() {
    let healthy = Arc::new(MemoryProvider::new());
    seed_empty_shard_map(healthy.as_ref()).await;
    let backends: Vec<Arc<dyn StorageProvider>> = vec![Arc::new(FailingProvider), healthy];
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
    seed_empty_shard_map(healthy.as_ref()).await;

    let backends: Vec<Arc<dyn StorageProvider>> = vec![Arc::new(FailingProvider), healthy.clone()];
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
        health: HealthConfig::default(),
    }
}

#[tokio::test]
async fn test_erasure_upload_download_basic() {
    let backends = make_backends(5);
    let provider = CompositeStorageProvider::new(backends.clone(), erasure_config(3, 2)).unwrap();

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
    let provider = CompositeStorageProvider::new(backends.clone(), erasure_config(2, 1)).unwrap();

    let path = VaultPath::parse("/empty.bin").unwrap();
    let data = vec![];

    provider.upload(&path, data.clone()).await.unwrap();
    let downloaded = provider.download(&path).await.unwrap();
    assert_eq!(downloaded, data);
}

#[tokio::test]
async fn test_erasure_upload_download_single_byte() {
    let backends = make_backends(3);
    let provider = CompositeStorageProvider::new(backends.clone(), erasure_config(2, 1)).unwrap();

    let path = VaultPath::parse("/one.bin").unwrap();
    let data = vec![42];

    provider.upload(&path, data.clone()).await.unwrap();
    let downloaded = provider.download(&path).await.unwrap();
    assert_eq!(downloaded, data);
}

#[tokio::test]
async fn test_erasure_upload_download_large_data() {
    let backends = make_backends(5);
    let provider = CompositeStorageProvider::new(backends.clone(), erasure_config(3, 2)).unwrap();

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
    let provider = CompositeStorageProvider::new(backends.clone(), erasure_config(3, 2)).unwrap();

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
    let provider = CompositeStorageProvider::new(backends.clone(), erasure_config(3, 2)).unwrap();

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
    let provider = CompositeStorageProvider::new(backends.clone(), erasure_config(2, 1)).unwrap();

    let path = VaultPath::parse("/check.bin").unwrap();
    assert!(!provider.exists(&path).await.unwrap());

    provider.upload(&path, vec![1, 2, 3]).await.unwrap();
    assert!(provider.exists(&path).await.unwrap());
}

#[tokio::test]
async fn test_erasure_delete() {
    let backends = make_backends(3);
    let provider = CompositeStorageProvider::new(backends.clone(), erasure_config(2, 1)).unwrap();

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
    let provider = CompositeStorageProvider::new(backends.clone(), erasure_config(2, 1)).unwrap();

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
    let provider = CompositeStorageProvider::new(backends.clone(), erasure_config(2, 1)).unwrap();

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
    let provider = CompositeStorageProvider::new(backends.clone(), erasure_config(2, 1)).unwrap();

    let path = VaultPath::parse("/meta.bin").unwrap();
    provider.upload(&path, vec![1, 2, 3]).await.unwrap();

    let meta = provider.metadata(&path).await.unwrap();
    assert_eq!(meta.name, "meta.bin");
    assert!(!meta.is_directory);
}

#[tokio::test]
async fn test_erasure_stream_upload_download() {
    let backends = make_backends(3);
    let provider = CompositeStorageProvider::new(backends.clone(), erasure_config(2, 1)).unwrap();

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
    let provider = CompositeStorageProvider::new(backends.clone(), erasure_config(3, 2)).unwrap();

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
            let shard_path = VaultPath::parse(&format!("{}.shard{}", path_str, shard_idx)).unwrap();
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
    let provider = CompositeStorageProvider::new(backends.clone(), erasure_config(3, 1)).unwrap();

    let path = VaultPath::parse("/odd.bin").unwrap();
    // 7 bytes / 3 shards = 2 remainder 1 → tests padding correctness
    let data = vec![1, 2, 3, 4, 5, 6, 7];

    provider.upload(&path, data.clone()).await.unwrap();
    let downloaded = provider.download(&path).await.unwrap();
    assert_eq!(downloaded, data);
}

#[tokio::test]
async fn test_erasure_corrupted_shard_detected() {
    // 3 data + 2 parity = 5 backends. Corrupt one shard payload.
    let backends = make_backends(5);
    let provider = CompositeStorageProvider::new(backends.clone(), erasure_config(3, 2)).unwrap();

    let path = VaultPath::parse("/corrupt.bin").unwrap();
    let data = b"data that must survive a corrupted shard".to_vec();

    provider.upload(&path, data.clone()).await.unwrap();

    // Corrupt shard 0 by overwriting with garbage
    let shard0_path = VaultPath::parse("/corrupt.bin.shard0").unwrap();
    backends[0]
        .upload(
            &shard0_path,
            vec![
                0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF,
            ],
        )
        .await
        .unwrap();

    // Download should still succeed — corrupt shard detected via CRC, RS reconstructs
    let downloaded = provider.download(&path).await.unwrap();
    assert_eq!(downloaded, data);
}

#[tokio::test]
async fn test_erasure_list_filters_shard_files() {
    let backends = make_backends(3);
    let provider = CompositeStorageProvider::new(backends.clone(), erasure_config(2, 1)).unwrap();

    let dir = VaultPath::parse("/data").unwrap();
    // Create dir on all backends so list works
    for backend in &backends {
        backend.create_dir(&dir).await.unwrap();
    }

    let path = VaultPath::parse("/data/file.txt").unwrap();
    provider.upload(&path, vec![1, 2, 3]).await.unwrap();

    let listing = provider.list(&dir).await.unwrap();
    // No .shardN entries should be visible
    for entry in &listing {
        assert!(
            !is_shard_file(&entry.name),
            "Shard file leaked into list: {}",
            entry.name
        );
    }
    // The listing should be empty since the logical file name "file.txt" is
    // not stored — only shard files exist on disk in erasure mode.
    assert!(listing.is_empty());
}

#[tokio::test]
async fn test_erasure_metadata_returns_original_size() {
    let backends = make_backends(3);
    let provider = CompositeStorageProvider::new(backends.clone(), erasure_config(2, 1)).unwrap();

    let path = VaultPath::parse("/sized.bin").unwrap();
    let data = vec![10, 20, 30, 40, 50];

    provider.upload(&path, data.clone()).await.unwrap();

    let meta = provider.metadata(&path).await.unwrap();
    assert_eq!(meta.name, "sized.bin");
    assert_eq!(meta.size, Some(data.len() as u64));
    assert!(!meta.is_directory);
}

#[test]
fn test_is_shard_file() {
    assert!(is_shard_file("file.txt.shard0"));
    assert!(is_shard_file("file.txt.shard12"));
    assert!(!is_shard_file("file.txt"));
    assert!(!is_shard_file("file.shard"));
    assert!(!is_shard_file("file.shardX"));
    assert!(!is_shard_file("noshard"));
}

// -- Toggleable provider for health tests ---------------------------------

use std::sync::atomic::{AtomicBool, Ordering};

/// A provider that delegates to an inner `MemoryProvider` but can be toggled
/// to fail all operations on demand.
struct ToggleableProvider {
    inner: MemoryProvider,
    should_fail: AtomicBool,
    provider_name: String,
}

impl ToggleableProvider {
    fn new(name: &str) -> Self {
        Self {
            inner: MemoryProvider::new(),
            should_fail: AtomicBool::new(false),
            provider_name: name.to_string(),
        }
    }

    fn set_failing(&self, fail: bool) {
        self.should_fail.store(fail, Ordering::SeqCst);
    }

    fn check(&self) -> Result<()> {
        if self.should_fail.load(Ordering::SeqCst) {
            Err(Error::Storage("backend down".into()))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl StorageProvider for ToggleableProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }
    async fn upload(&self, path: &VaultPath, data: Vec<u8>) -> Result<Metadata> {
        self.check()?;
        self.inner.upload(path, data).await
    }
    async fn upload_stream(&self, path: &VaultPath, stream: ByteStream) -> Result<Metadata> {
        self.check()?;
        self.inner.upload_stream(path, stream).await
    }
    async fn download(&self, path: &VaultPath) -> Result<Vec<u8>> {
        self.check()?;
        self.inner.download(path).await
    }
    async fn download_stream(&self, path: &VaultPath) -> Result<ByteStream> {
        self.check()?;
        self.inner.download_stream(path).await
    }
    async fn exists(&self, path: &VaultPath) -> Result<bool> {
        self.check()?;
        self.inner.exists(path).await
    }
    async fn delete(&self, path: &VaultPath) -> Result<()> {
        self.check()?;
        self.inner.delete(path).await
    }
    async fn list(&self, path: &VaultPath) -> Result<Vec<Metadata>> {
        self.check()?;
        self.inner.list(path).await
    }
    async fn metadata(&self, path: &VaultPath) -> Result<Metadata> {
        self.check()?;
        self.inner.metadata(path).await
    }
    async fn create_dir(&self, path: &VaultPath) -> Result<Metadata> {
        self.check()?;
        self.inner.create_dir(path).await
    }
    async fn delete_dir(&self, path: &VaultPath) -> Result<()> {
        self.check()?;
        self.inner.delete_dir(path).await
    }
    async fn rename(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        self.check()?;
        self.inner.rename(from, to).await
    }
    async fn copy(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        self.check()?;
        self.inner.copy(from, to).await
    }
}

// -- Health tracking tests ------------------------------------------------

#[tokio::test]
async fn test_health_backends_start_healthy() {
    let provider = CompositeStorageProvider::new(make_backends(3), mirror_config()).unwrap();
    assert_eq!(provider.healthy_backend_count().await, 3);
    for i in 0..3 {
        let h = provider.backend_health(i).await.unwrap();
        assert_eq!(h.status, HealthStatus::Healthy);
    }
}

#[tokio::test]
async fn test_health_degraded_after_consecutive_failures() {
    let b0 = Arc::new(ToggleableProvider::new("t0"));
    let b1 = Arc::new(ToggleableProvider::new("t1"));
    let b2 = Arc::new(ToggleableProvider::new("t2"));

    // Upload some data while all backends are healthy
    let backends: Vec<Arc<dyn StorageProvider>> = vec![b0.clone(), b1.clone(), b2.clone()];
    let config = CompositeConfig {
        mode: RaidMode::Mirror,
        health: HealthConfig {
            failure_threshold: 3,
            unhealthy_threshold: 10,
            recovery_interval_secs: 3600,
        },
    };
    let provider = CompositeStorageProvider::new(backends, config).unwrap();

    let path = VaultPath::parse("/test.txt").unwrap();
    provider.upload(&path, b"hello".to_vec()).await.unwrap();

    // Now make b0 fail
    b0.set_failing(true);

    // Trigger 3 write failures on b0 (fan_out writes to all, so b0 fails each time)
    for _ in 0..3 {
        provider.upload(&path, b"hello".to_vec()).await.unwrap();
    }

    let h0 = provider.backend_health(0).await.unwrap();
    assert_eq!(h0.status, HealthStatus::Degraded);
    assert_eq!(h0.consecutive_failures, 3);

    // Backends 1 and 2 should still be healthy
    let h1 = provider.backend_health(1).await.unwrap();
    assert_eq!(h1.status, HealthStatus::Healthy);
    let h2 = provider.backend_health(2).await.unwrap();
    assert_eq!(h2.status, HealthStatus::Healthy);

    assert_eq!(provider.healthy_backend_count().await, 2);
}

#[tokio::test]
async fn test_health_degraded_skipped_for_reads() {
    let b0 = Arc::new(ToggleableProvider::new("t0"));
    let b1 = Arc::new(ToggleableProvider::new("t1"));

    let backends: Vec<Arc<dyn StorageProvider>> = vec![b0.clone(), b1.clone()];
    let config = CompositeConfig {
        mode: RaidMode::Mirror,
        health: HealthConfig {
            failure_threshold: 2,
            unhealthy_threshold: 10,
            recovery_interval_secs: 3600,
        },
    };
    let provider = CompositeStorageProvider::new(backends, config).unwrap();

    let path = VaultPath::parse("/data.bin").unwrap();
    provider.upload(&path, b"data".to_vec()).await.unwrap();

    // Make b0 fail and trigger enough failures to degrade it
    b0.set_failing(true);
    for _ in 0..2 {
        // These writes succeed overall (b1 is healthy) but record failures for b0
        provider.upload(&path, b"data".to_vec()).await.unwrap();
    }

    let h0 = provider.backend_health(0).await.unwrap();
    assert_eq!(h0.status, HealthStatus::Degraded);

    // Download should still succeed via b1 (b0 is skipped)
    let data = provider.download(&path).await.unwrap();
    assert_eq!(data, b"data");
}

#[tokio::test]
async fn test_health_recovery_on_success() {
    let b0 = Arc::new(ToggleableProvider::new("t0"));
    let b1 = Arc::new(ToggleableProvider::new("t1"));

    let backends: Vec<Arc<dyn StorageProvider>> = vec![b0.clone(), b1.clone()];
    let config = CompositeConfig {
        mode: RaidMode::Mirror,
        health: HealthConfig {
            failure_threshold: 2,
            unhealthy_threshold: 10,
            recovery_interval_secs: 3600,
        },
    };
    let provider = CompositeStorageProvider::new(backends, config).unwrap();

    let path = VaultPath::parse("/recover.txt").unwrap();
    provider.upload(&path, b"v1".to_vec()).await.unwrap();

    // Degrade b0
    b0.set_failing(true);
    for _ in 0..2 {
        provider.upload(&path, b"v1".to_vec()).await.unwrap();
    }
    assert_eq!(
        provider.backend_health(0).await.unwrap().status,
        HealthStatus::Degraded
    );

    // Restore b0
    b0.set_failing(false);

    // A successful write should recover it
    provider.upload(&path, b"v2".to_vec()).await.unwrap();
    assert_eq!(
        provider.backend_health(0).await.unwrap().status,
        HealthStatus::Healthy
    );
    assert_eq!(provider.healthy_backend_count().await, 2);
}

#[tokio::test]
async fn test_health_transition_to_offline() {
    let b0 = Arc::new(ToggleableProvider::new("t0"));
    let b1 = Arc::new(ToggleableProvider::new("t1"));

    let backends: Vec<Arc<dyn StorageProvider>> = vec![b0.clone(), b1.clone()];
    let config = CompositeConfig {
        mode: RaidMode::Mirror,
        health: HealthConfig {
            failure_threshold: 2,
            unhealthy_threshold: 5,
            recovery_interval_secs: 3600,
        },
    };
    let provider = CompositeStorageProvider::new(backends, config).unwrap();

    let path = VaultPath::parse("/offline.txt").unwrap();
    provider.upload(&path, b"data".to_vec()).await.unwrap();

    b0.set_failing(true);

    // 5 consecutive failures should transition through Degraded -> Offline
    for _ in 0..5 {
        provider.upload(&path, b"data".to_vec()).await.unwrap();
    }

    let h0 = provider.backend_health(0).await.unwrap();
    assert_eq!(h0.status, HealthStatus::Unhealthy);
    assert_eq!(h0.consecutive_failures, 5);
}

#[tokio::test]
async fn test_health_all_degraded_fallback_reads() {
    // When all backends are degraded, reads should still try all (prevent lockout)
    let b0 = Arc::new(ToggleableProvider::new("t0"));
    let b1 = Arc::new(ToggleableProvider::new("t1"));

    let backends: Vec<Arc<dyn StorageProvider>> = vec![b0.clone(), b1.clone()];
    let config = CompositeConfig {
        mode: RaidMode::Mirror,
        health: HealthConfig {
            failure_threshold: 1,
            unhealthy_threshold: 10,
            recovery_interval_secs: 3600,
        },
    };
    let provider = CompositeStorageProvider::new(backends, config).unwrap();

    let path = VaultPath::parse("/fallback.txt").unwrap();
    provider.upload(&path, b"data".to_vec()).await.unwrap();

    // Make both backends fail once each to degrade them
    b0.set_failing(true);
    b1.set_failing(true);
    let _ = provider.upload(&path, b"data".to_vec()).await; // Both fail

    // Now restore both
    b0.set_failing(false);
    b1.set_failing(false);

    // Both are degraded, but healthy_backend_indices falls back to all
    assert_eq!(provider.healthy_backend_count().await, 0);

    // Download should succeed because fallback tries all backends
    let data = provider.download(&path).await.unwrap();
    assert_eq!(data, b"data");
}

#[tokio::test]
async fn test_health_intermittent_failures_dont_degrade() {
    let b0 = Arc::new(ToggleableProvider::new("t0"));
    let b1 = Arc::new(ToggleableProvider::new("t1"));

    let backends: Vec<Arc<dyn StorageProvider>> = vec![b0.clone(), b1.clone()];
    let config = CompositeConfig {
        mode: RaidMode::Mirror,
        health: HealthConfig {
            failure_threshold: 3,
            unhealthy_threshold: 10,
            recovery_interval_secs: 3600,
        },
    };
    let provider = CompositeStorageProvider::new(backends, config).unwrap();

    let path = VaultPath::parse("/intermittent.txt").unwrap();
    provider.upload(&path, b"v1".to_vec()).await.unwrap();

    // 2 failures on b0
    b0.set_failing(true);
    provider.upload(&path, b"v1".to_vec()).await.unwrap();
    provider.upload(&path, b"v1".to_vec()).await.unwrap();

    // Success resets counter
    b0.set_failing(false);
    provider.upload(&path, b"v1".to_vec()).await.unwrap();

    assert_eq!(
        provider.backend_health(0).await.unwrap().status,
        HealthStatus::Healthy
    );
    assert_eq!(
        provider
            .backend_health(0)
            .await
            .unwrap()
            .consecutive_failures,
        0
    );
}

#[tokio::test]
async fn test_health_latency_tracking() {
    let provider = CompositeStorageProvider::new(make_backends(2), mirror_config()).unwrap();
    let path = VaultPath::parse("/latency.txt").unwrap();
    provider.upload(&path, b"data".to_vec()).await.unwrap();

    let h = provider.backend_health(0).await.unwrap();
    // After at least one successful op, avg_latency_ms should be > 0
    assert!(h.avg_latency_ms > 0.0);
}

#[tokio::test]
async fn test_health_probe_recovers_degraded_backend() {
    let b0 = Arc::new(ToggleableProvider::new("t0"));
    let b1 = Arc::new(ToggleableProvider::new("t1"));

    let backends: Vec<Arc<dyn StorageProvider>> = vec![b0.clone(), b1.clone()];
    let config = CompositeConfig {
        mode: RaidMode::Mirror,
        health: HealthConfig {
            failure_threshold: 1,
            unhealthy_threshold: 10,
            // Very short recovery interval so the probe fires immediately
            recovery_interval_secs: 0,
        },
    };
    let provider = CompositeStorageProvider::new(backends, config).unwrap();

    let path = VaultPath::parse("/probe.txt").unwrap();
    provider.upload(&path, b"data".to_vec()).await.unwrap();

    // Degrade b0
    b0.set_failing(true);
    provider.upload(&path, b"data".to_vec()).await.unwrap();
    assert_eq!(
        provider.backend_health(0).await.unwrap().status,
        HealthStatus::Degraded
    );

    // Restore b0
    b0.set_failing(false);

    // The next read triggers probe_if_due(), which should recover b0
    let data = provider.download(&path).await.unwrap();
    assert_eq!(data, b"data");

    assert_eq!(
        provider.backend_health(0).await.unwrap().status,
        HealthStatus::Healthy
    );
}

#[tokio::test]
async fn test_config_serde_backward_compat() {
    // Old-format JSON without health field should deserialize with defaults
    let json = r#"{"mode":"Mirror"}"#;
    let config: CompositeConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.mode, RaidMode::Mirror);
    assert_eq!(config.health.failure_threshold, 3);
    assert_eq!(config.health.unhealthy_threshold, 10);
    assert_eq!(config.health.recovery_interval_secs, 60);
}
