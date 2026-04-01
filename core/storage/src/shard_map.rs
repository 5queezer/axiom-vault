//! Shard map: persistent metadata for shard-to-backend mapping.
//!
//! Tracks which shard of which chunk lives on which backend. In mirror mode
//! this is trivial (all chunks on all backends), but in erasure mode the map
//! records shard index, backend ID, chunk path, and encoding parameters.
//!
//! The shard map is stored redundantly on every backend at a well-known path
//! (`.axiomvault/shard_map.json`), versioned with a monotonic counter, and
//! updated atomically via write-then-rename.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::warn;

use crate::provider::StorageProvider;
use axiomvault_common::{Error, Result, VaultPath};

/// Well-known path where the shard map is stored on each backend.
const SHARD_MAP_PATH: &str = ".axiomvault/shard_map.json";

/// Temporary path used for atomic writes (write then rename).
const SHARD_MAP_TMP_PATH: &str = ".axiomvault/shard_map.json.tmp";

/// Directory that must exist before writing the shard map.
const SHARD_MAP_DIR: &str = ".axiomvault";

/// Encoding parameters for a chunk's erasure coding scheme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErasureParams {
    /// Number of data shards.
    pub data_shards: usize,
    /// Number of parity shards.
    pub parity_shards: usize,
}

/// Where a single shard is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShardLocation {
    /// Index of the shard within the chunk's shard set.
    pub shard_index: usize,
    /// Identifier of the backend holding this shard (typically `StorageProvider::name()`
    /// plus an index for disambiguation when multiple backends share a name).
    pub backend_id: String,
    /// Path on the backend where the shard is stored.
    pub backend_path: String,
    /// Whether this is a parity shard (true) or data shard (false).
    pub is_parity: bool,
}

/// Metadata for all shards belonging to a single chunk (file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkEntry {
    /// Original size of the data before encoding.
    pub original_size: u64,
    /// Erasure coding parameters (None for mirror mode).
    pub erasure_params: Option<ErasureParams>,
    /// Shard locations keyed by shard index.
    pub shards: HashMap<usize, ShardLocation>,
    /// Timestamp of when this entry was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Persistent shard map tracking chunk-to-shard-to-backend mappings.
///
/// Versioned with a monotonic counter for conflict resolution across backends.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardMap {
    /// Monotonically increasing version counter.
    pub version: u64,
    /// Chunk entries keyed by logical file path (e.g. `/data/encrypted_name`).
    pub entries: HashMap<String, ChunkEntry>,
    /// Timestamp of the last modification to this shard map.
    pub updated_at: DateTime<Utc>,
    /// Tombstones for deleted entries, keyed by path with deletion timestamp.
    ///
    /// Prevents deleted entries from being resurrected when merging with stale backends.
    #[serde(default)]
    pub tombstones: HashMap<String, DateTime<Utc>>,
}

impl ShardMap {
    /// Create a new, empty shard map.
    pub fn new() -> Self {
        Self {
            version: 0,
            entries: HashMap::new(),
            updated_at: Utc::now(),
            tombstones: HashMap::new(),
        }
    }

    /// Record a chunk entry for the given path.
    ///
    /// Bumps the version counter and updates the timestamp.
    pub fn insert(&mut self, path: &str, entry: ChunkEntry) {
        self.version += 1;
        self.updated_at = Utc::now();
        // Clear any stale tombstone so merge doesn't suppress this entry.
        self.tombstones.remove(path);
        self.entries.insert(path.to_string(), entry);
    }

    /// Remove a chunk entry. Returns the removed entry if it existed.
    ///
    /// Bumps the version counter, updates the timestamp, and records a tombstone
    /// to prevent the entry from being resurrected when merging with stale backends.
    pub fn remove(&mut self, path: &str) -> Option<ChunkEntry> {
        let removed = self.entries.remove(path);
        // Always refresh the tombstone so repeated deletes advance the timestamp,
        // ensuring stale entries on diverged backends are suppressed on merge.
        let now = Utc::now();
        self.version += 1;
        self.updated_at = now;
        self.tombstones.insert(path.to_string(), now);
        removed
    }

    /// Update entry for a rename operation.
    ///
    /// Moves the entry from `from` to `to`, updating shard backend paths.
    pub fn rename(&mut self, from: &str, to: &str) -> Option<ChunkEntry> {
        if let Some(mut entry) = self.entries.remove(from) {
            // Update backend_path in each shard: replace only the leading
            // `from` prefix to avoid accidental substring replacements.
            for shard in entry.shards.values_mut() {
                if let Some(suffix) = shard.backend_path.strip_prefix(from) {
                    shard.backend_path = format!("{}{}", to, suffix);
                }
            }
            let now = Utc::now();
            entry.updated_at = now;
            self.version += 1;
            self.updated_at = now;
            // Tombstone the old path to prevent resurrection from stale backends.
            self.tombstones.insert(from.to_string(), now);
            // Clear any stale tombstone on the destination path.
            self.tombstones.remove(to);
            self.entries.insert(to.to_string(), entry.clone());
            Some(entry)
        } else {
            None
        }
    }

    /// Look up a chunk entry by path.
    pub fn get(&self, path: &str) -> Option<&ChunkEntry> {
        self.entries.get(path)
    }

    /// Serialize the shard map to JSON bytes.
    pub fn to_json(&self) -> Result<Vec<u8>> {
        serde_json::to_vec_pretty(self)
            .map_err(|e| Error::Serialization(format!("Failed to serialize shard map: {}", e)))
    }

    /// Deserialize a shard map from JSON bytes.
    pub fn from_json(data: &[u8]) -> Result<Self> {
        serde_json::from_slice(data)
            .map_err(|e| Error::Serialization(format!("Failed to deserialize shard map: {}", e)))
    }

    /// Merge another shard map into this one.
    ///
    /// For each chunk entry, the entry with the newer `updated_at` timestamp wins.
    /// Tombstones are merged (newest per path wins) and suppress resurrection of
    /// deleted entries. The resulting version is the maximum of both versions plus one.
    pub fn merge(&mut self, other: &ShardMap) {
        // Merge tombstones: keep the newest deletion timestamp per path.
        for (path, &other_ts) in &other.tombstones {
            let self_ts = self.tombstones.get(path).copied();
            if self_ts.is_none_or(|t| other_ts > t) {
                self.tombstones.insert(path.clone(), other_ts);
            }
        }

        // Merge entries: tombstones take precedence over older entries.
        for (path, other_entry) in &other.entries {
            // If self has a tombstone newer than this entry, skip it (keep deleted).
            if let Some(&tomb_ts) = self.tombstones.get(path) {
                if tomb_ts >= other_entry.updated_at {
                    continue;
                }
                // other_entry is newer than the tombstone — resurrect and clear tombstone.
                self.tombstones.remove(path);
            }
            match self.entries.get(path) {
                Some(existing) if existing.updated_at >= other_entry.updated_at => {
                    // Keep existing (same or newer)
                }
                _ => {
                    self.entries.insert(path.clone(), other_entry.clone());
                }
            }
        }

        // Remove self's entries that other has a newer tombstone for.
        for (path, &other_ts) in &other.tombstones {
            if let Some(existing) = self.entries.get(path) {
                if other_ts >= existing.updated_at {
                    self.entries.remove(path);
                }
            }
        }

        self.version = self.version.max(other.version) + 1;
        self.updated_at = Utc::now();
    }

    /// Save the shard map to a single backend atomically (write temp, then rename).
    pub async fn save_to_backend(&self, backend: &dyn StorageProvider) -> Result<()> {
        let dir_path = VaultPath::parse(SHARD_MAP_DIR)?;
        let tmp_path = VaultPath::parse(SHARD_MAP_TMP_PATH)?;
        let final_path = VaultPath::parse(SHARD_MAP_PATH)?;

        // Ensure directory exists
        if !backend.exists(&dir_path).await.unwrap_or(false) {
            backend.create_dir(&dir_path).await?;
        }

        let data = self.to_json()?;

        // Write to temp path first (overwrite any leftover tmp from a previous attempt)
        if backend.exists(&tmp_path).await.unwrap_or(false) {
            let _ = backend.delete(&tmp_path).await;
        }
        backend.upload(&tmp_path, data).await?;

        // Try rename; if it fails (e.g. destination exists), delete final and retry.
        // This avoids deleting the canonical file before the rename succeeds,
        // which would leave the backend without a shard map on rename failure.
        match backend.rename(&tmp_path, &final_path).await {
            Ok(_) => {}
            Err(_) => {
                let _ = backend.delete(&final_path).await;
                backend.rename(&tmp_path, &final_path).await?;
            }
        }

        Ok(())
    }

    /// Save the shard map redundantly to all provided backends concurrently.
    ///
    /// Tolerates partial failures — warns but continues if some backends fail.
    /// Returns error only if ALL backends fail.
    pub async fn save_to_all(&self, backends: &[Arc<dyn StorageProvider>]) -> Result<()> {
        let futures: Vec<_> = backends
            .iter()
            .enumerate()
            .map(|(i, backend)| {
                let backend = Arc::clone(backend);
                let map = self.clone();
                async move {
                    (
                        i,
                        backend.name().to_string(),
                        map.save_to_backend(backend.as_ref()).await,
                    )
                }
            })
            .collect();

        let results = futures::future::join_all(futures).await;

        let mut any_success = false;
        let mut last_error: Option<Error> = None;

        for (i, backend_name, result) in results {
            match result {
                Ok(()) => any_success = true,
                Err(e) => {
                    warn!(
                        backend = backend_name,
                        index = i,
                        error = %e,
                        "Failed to save shard map to backend"
                    );
                    last_error = Some(e);
                }
            }
        }

        if any_success {
            Ok(())
        } else {
            Err(last_error.unwrap_or_else(|| {
                Error::Storage("Failed to save shard map to any backend".into())
            }))
        }
    }

    /// Load shard map from a single backend. Returns `None` if not found.
    pub async fn load_from_backend(backend: &dyn StorageProvider) -> Result<Option<ShardMap>> {
        let path = VaultPath::parse(SHARD_MAP_PATH)?;

        match backend.download(&path).await {
            Ok(data) => {
                let map = Self::from_json(&data)?;
                Ok(Some(map))
            }
            Err(Error::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Load and merge shard maps from all backends.
    ///
    /// Fetches the shard map from each backend, then merges them using
    /// per-entry latest-timestamp-wins semantics. The resulting version is the
    /// maximum found across all backends plus one.
    ///
    /// Returns a new empty shard map if no backend has a stored map and none errored.
    /// Returns an error if no actual map was loaded (`Ok(Some(_))`) AND at least one
    /// backend returned an error. `Ok(None)` (not found) does not suppress errors
    /// from other backends.
    pub async fn load_from_all(backends: &[Arc<dyn StorageProvider>]) -> Result<ShardMap> {
        let mut merged = ShardMap::new();
        let mut found_any = false;
        let mut last_error: Option<Error> = None;

        for (i, backend) in backends.iter().enumerate() {
            match Self::load_from_backend(backend.as_ref()).await {
                Ok(Some(map)) => {
                    if !found_any {
                        merged = map;
                        found_any = true;
                    } else {
                        merged.merge(&map);
                    }
                }
                Ok(None) => {
                    // No shard map on this backend yet — skip
                }
                Err(e) => {
                    warn!(
                        backend = backend.name(),
                        index = i,
                        error = %e,
                        "Failed to load shard map from backend, skipping"
                    );
                    last_error = Some(e);
                }
            }
        }

        // Only error if no actual map was loaded AND at least one backend errored.
        // Ok(None) (not found) does not suppress errors from other backends.
        if !found_any {
            if let Some(e) = last_error {
                return Err(e);
            }
        }

        Ok(merged)
    }

    /// Build a `ChunkEntry` for a mirror-mode chunk.
    /// Build a `ChunkEntry` for a mirror-mode chunk.
    ///
    /// If `succeeded` is `Some`, only the given backend indices are recorded.
    /// If `None`, all backends are included (used when no partial failure tracking).
    pub fn mirror_entry(
        path: &str,
        original_size: u64,
        backends: &[Arc<dyn StorageProvider>],
        succeeded: Option<&[usize]>,
    ) -> ChunkEntry {
        let indices: Vec<usize> = match succeeded {
            Some(s) => s.to_vec(),
            None => (0..backends.len()).collect(),
        };
        let mut shards = HashMap::new();
        for i in indices {
            let backend_id = if i < backends.len() {
                format!("{}:{}", backends[i].name(), i)
            } else {
                format!("unknown:{}", i)
            };
            shards.insert(
                i,
                ShardLocation {
                    shard_index: i,
                    backend_id,
                    backend_path: path.to_string(),
                    is_parity: false,
                },
            );
        }
        ChunkEntry {
            original_size,
            erasure_params: None,
            shards,
            updated_at: Utc::now(),
        }
    }

    /// Build a `ChunkEntry` for an erasure-coded chunk.
    /// Build a `ChunkEntry` for an erasure-coded chunk.
    ///
    /// If `succeeded` is `Some`, only the given shard indices are recorded.
    /// If `None`, all shards (0..data+parity) are included.
    pub fn erasure_entry(
        path: &str,
        original_size: u64,
        data_shards: usize,
        parity_shards: usize,
        backends: &[Arc<dyn StorageProvider>],
        succeeded: Option<&[usize]>,
    ) -> ChunkEntry {
        let total = data_shards + parity_shards;
        let indices: Vec<usize> = match succeeded {
            Some(s) => s.to_vec(),
            None => (0..total).collect(),
        };
        let mut shards = HashMap::new();
        for i in indices {
            let backend_id = if i < backends.len() {
                format!("{}:{}", backends[i].name(), i)
            } else {
                format!("unknown:{}", i)
            };
            shards.insert(
                i,
                ShardLocation {
                    shard_index: i,
                    backend_id,
                    backend_path: format!("{}.shard{}", path, i),
                    is_parity: i >= data_shards,
                },
            );
        }
        ChunkEntry {
            original_size,
            erasure_params: Some(ErasureParams {
                data_shards,
                parity_shards,
            }),
            shards,
            updated_at: Utc::now(),
        }
    }
}

impl Default for ShardMap {
    fn default() -> Self {
        Self::new()
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

    // -- Serialization roundtrip tests ----------------------------------------

    #[test]
    fn test_empty_shard_map_roundtrip() {
        let map = ShardMap::new();
        let json = map.to_json().unwrap();
        let decoded = ShardMap::from_json(&json).unwrap();
        assert_eq!(decoded.version, 0);
        assert!(decoded.entries.is_empty());
    }

    #[test]
    fn test_shard_map_with_entries_roundtrip() {
        let mut map = ShardMap::new();
        let entry = ChunkEntry {
            original_size: 1024,
            erasure_params: Some(ErasureParams {
                data_shards: 3,
                parity_shards: 2,
            }),
            shards: {
                let mut s = HashMap::new();
                for i in 0..5 {
                    s.insert(
                        i,
                        ShardLocation {
                            shard_index: i,
                            backend_id: format!("backend:{}", i),
                            backend_path: format!("/data/file.enc.shard{}", i),
                            is_parity: i >= 3,
                        },
                    );
                }
                s
            },
            updated_at: Utc::now(),
        };
        map.insert("/data/file.enc", entry.clone());

        let json = map.to_json().unwrap();
        let decoded = ShardMap::from_json(&json).unwrap();

        assert_eq!(decoded.version, 1);
        assert_eq!(decoded.entries.len(), 1);
        let decoded_entry = decoded.get("/data/file.enc").unwrap();
        assert_eq!(decoded_entry.original_size, 1024);
        assert_eq!(decoded_entry.shards.len(), 5);
        assert_eq!(
            decoded_entry.erasure_params,
            Some(ErasureParams {
                data_shards: 3,
                parity_shards: 2,
            })
        );
    }

    #[test]
    fn test_mirror_entry_roundtrip() {
        let backends = make_backends(3);
        let entry = ShardMap::mirror_entry("/data/file.enc", 512, &backends, None);

        assert!(entry.erasure_params.is_none());
        assert_eq!(entry.original_size, 512);
        assert_eq!(entry.shards.len(), 3);

        for (i, shard) in &entry.shards {
            assert_eq!(shard.shard_index, *i);
            assert_eq!(shard.backend_path, "/data/file.enc");
            assert!(!shard.is_parity);
        }

        // Roundtrip through JSON
        let mut map = ShardMap::new();
        map.insert("/data/file.enc", entry);
        let json = map.to_json().unwrap();
        let decoded = ShardMap::from_json(&json).unwrap();
        assert_eq!(decoded.entries.len(), 1);
    }

    #[test]
    fn test_erasure_entry_roundtrip() {
        let backends = make_backends(5);
        let entry = ShardMap::erasure_entry("/data/file.enc", 2048, 3, 2, &backends, None);

        assert_eq!(
            entry.erasure_params,
            Some(ErasureParams {
                data_shards: 3,
                parity_shards: 2,
            })
        );
        assert_eq!(entry.original_size, 2048);
        assert_eq!(entry.shards.len(), 5);

        // First 3 shards are data, last 2 are parity
        for i in 0..3 {
            assert!(!entry.shards[&i].is_parity);
            assert_eq!(
                entry.shards[&i].backend_path,
                format!("/data/file.enc.shard{}", i)
            );
        }
        for i in 3..5 {
            assert!(entry.shards[&i].is_parity);
        }
    }

    // -- Insert / remove / rename tests ---------------------------------------

    #[test]
    fn test_insert_bumps_version() {
        let mut map = ShardMap::new();
        assert_eq!(map.version, 0);

        let backends = make_backends(2);
        map.insert("/a", ShardMap::mirror_entry("/a", 100, &backends, None));
        assert_eq!(map.version, 1);

        map.insert("/b", ShardMap::mirror_entry("/b", 200, &backends, None));
        assert_eq!(map.version, 2);
    }

    #[test]
    fn test_remove_always_records_tombstone() {
        let mut map = ShardMap::new();
        let backends = make_backends(2);
        map.insert("/a", ShardMap::mirror_entry("/a", 100, &backends, None));
        assert_eq!(map.version, 1);

        // Remove nonexistent — still bumps version and records tombstone
        assert!(map.remove("/nonexistent").is_none());
        assert_eq!(map.version, 2);
        assert!(map.tombstones.contains_key("/nonexistent"));

        // Remove existing
        assert!(map.remove("/a").is_some());
        assert_eq!(map.version, 3);
        assert!(map.entries.is_empty());
        assert!(map.tombstones.contains_key("/a"));
    }

    #[test]
    fn test_rename_entry() {
        let mut map = ShardMap::new();
        let backends = make_backends(5);
        map.insert(
            "/data/old.enc",
            ShardMap::erasure_entry("/data/old.enc", 1024, 3, 2, &backends, None),
        );

        let renamed = map.rename("/data/old.enc", "/data/new.enc");
        assert!(renamed.is_some());
        assert!(map.get("/data/old.enc").is_none());

        let entry = map.get("/data/new.enc").unwrap();
        for shard in entry.shards.values() {
            assert!(
                shard.backend_path.contains("/data/new.enc"),
                "Shard path should be updated: {}",
                shard.backend_path
            );
        }
    }

    #[test]
    fn test_rename_nonexistent_returns_none() {
        let mut map = ShardMap::new();
        assert!(map.rename("/nope", "/also_nope").is_none());
    }

    // -- Merge / conflict resolution tests ------------------------------------

    #[test]
    fn test_merge_union_of_entries() {
        let backends = make_backends(2);
        let mut map_a = ShardMap::new();
        map_a.insert("/a", ShardMap::mirror_entry("/a", 100, &backends, None));

        let mut map_b = ShardMap::new();
        map_b.insert("/b", ShardMap::mirror_entry("/b", 200, &backends, None));

        map_a.merge(&map_b);

        assert!(map_a.get("/a").is_some());
        assert!(map_a.get("/b").is_some());
        assert_eq!(map_a.entries.len(), 2);
    }

    #[test]
    fn test_merge_newer_entry_wins() {
        let backends = make_backends(2);

        let mut map_a = ShardMap::new();
        let old_entry = ChunkEntry {
            original_size: 100,
            erasure_params: None,
            shards: HashMap::new(),
            updated_at: Utc::now() - chrono::Duration::seconds(10),
        };
        map_a.entries.insert("/file".to_string(), old_entry);
        map_a.version = 1;

        let mut map_b = ShardMap::new();
        let new_entry = ShardMap::mirror_entry("/file", 200, &backends, None);
        map_b.entries.insert("/file".to_string(), new_entry);
        map_b.version = 2;

        map_a.merge(&map_b);

        // map_b's entry is newer, so it should win
        assert_eq!(map_a.get("/file").unwrap().original_size, 200);
    }

    #[test]
    fn test_merge_older_entry_does_not_overwrite() {
        let backends = make_backends(2);

        let mut map_a = ShardMap::new();
        let new_entry = ShardMap::mirror_entry("/file", 300, &backends, None);
        map_a.entries.insert("/file".to_string(), new_entry);
        map_a.version = 5;

        let mut map_b = ShardMap::new();
        let old_entry = ChunkEntry {
            original_size: 100,
            erasure_params: None,
            shards: HashMap::new(),
            updated_at: Utc::now() - chrono::Duration::seconds(60),
        };
        map_b.entries.insert("/file".to_string(), old_entry);
        map_b.version = 3;

        map_a.merge(&map_b);

        // map_a's entry is newer, should be kept
        assert_eq!(map_a.get("/file").unwrap().original_size, 300);
    }

    #[test]
    fn test_merge_version_is_max_plus_one() {
        let mut map_a = ShardMap::new();
        map_a.version = 5;

        let mut map_b = ShardMap::new();
        map_b.version = 10;

        map_a.merge(&map_b);
        assert_eq!(map_a.version, 11); // max(5, 10) + 1
    }

    // -- Persistence tests (save/load with MemoryProvider) --------------------

    #[tokio::test]
    async fn test_save_and_load_single_backend() {
        let backend = Arc::new(MemoryProvider::new());
        let backends: Vec<Arc<dyn StorageProvider>> = vec![backend.clone()];

        let mut map = ShardMap::new();
        map.insert(
            "/test",
            ShardMap::mirror_entry("/test", 42, &backends, None),
        );

        map.save_to_backend(backend.as_ref()).await.unwrap();

        let loaded = ShardMap::load_from_backend(backend.as_ref())
            .await
            .unwrap()
            .expect("shard map should exist");
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.get("/test").unwrap().original_size, 42);
    }

    #[tokio::test]
    async fn test_load_returns_none_when_missing() {
        let backend = Arc::new(MemoryProvider::new());
        let loaded = ShardMap::load_from_backend(backend.as_ref()).await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_save_to_all_backends() {
        let backends = make_backends(3);

        let mut map = ShardMap::new();
        map.insert(
            "/data/chunk1",
            ShardMap::mirror_entry("/data/chunk1", 100, &backends, None),
        );

        map.save_to_all(&backends).await.unwrap();

        // Verify each backend has the shard map
        for backend in &backends {
            let loaded = ShardMap::load_from_backend(backend.as_ref())
                .await
                .unwrap()
                .expect("shard map should exist on each backend");
            assert_eq!(loaded.version, 1);
            assert!(loaded.get("/data/chunk1").is_some());
        }
    }

    #[tokio::test]
    async fn test_load_and_merge_from_diverged_backends() {
        let backends = make_backends(3);

        // Backend 0 has entry A
        let mut map_0 = ShardMap::new();
        map_0.insert("/a", ShardMap::mirror_entry("/a", 100, &backends, None));
        map_0.save_to_backend(backends[0].as_ref()).await.unwrap();

        // Backend 1 has entry B
        let mut map_1 = ShardMap::new();
        map_1.insert("/b", ShardMap::mirror_entry("/b", 200, &backends, None));
        map_1.save_to_backend(backends[1].as_ref()).await.unwrap();

        // Backend 2 has no shard map

        let merged = ShardMap::load_from_all(&backends).await.unwrap();

        // Should have union of entries
        assert!(merged.get("/a").is_some());
        assert!(merged.get("/b").is_some());
        assert_eq!(merged.entries.len(), 2);
    }

    #[tokio::test]
    async fn test_load_from_all_with_no_maps() {
        let backends = make_backends(3);
        let map = ShardMap::load_from_all(&backends).await.unwrap();
        assert_eq!(map.version, 0);
        assert!(map.entries.is_empty());
    }

    #[tokio::test]
    async fn test_atomic_update_does_not_leave_tmp() {
        let backend: Arc<dyn StorageProvider> = Arc::new(MemoryProvider::new());

        let mut map = ShardMap::new();
        map.insert(
            "/test",
            ShardMap::mirror_entry("/test", 42, std::slice::from_ref(&backend), None),
        );
        map.save_to_backend(backend.as_ref()).await.unwrap();

        // Temp file should not exist after atomic rename
        let tmp_path = VaultPath::parse(SHARD_MAP_TMP_PATH).unwrap();
        assert!(!backend.exists(&tmp_path).await.unwrap());

        // Final file should exist
        let final_path = VaultPath::parse(SHARD_MAP_PATH).unwrap();
        assert!(backend.exists(&final_path).await.unwrap());
    }

    #[tokio::test]
    async fn test_conflict_resolution_highest_version_wins() {
        let backends = make_backends(2);

        // Backend 0: version 5, file X with size 100
        let mut map_old = ShardMap::new();
        map_old.version = 4; // will become 5 after insert
        let old_entry = ChunkEntry {
            original_size: 100,
            erasure_params: None,
            shards: HashMap::new(),
            updated_at: Utc::now() - chrono::Duration::seconds(30),
        };
        map_old.entries.insert("/x".to_string(), old_entry);
        map_old.version = 5;
        map_old.save_to_backend(backends[0].as_ref()).await.unwrap();

        // Backend 1: version 8, file X with size 200
        let mut map_new = ShardMap::new();
        let new_entry = ChunkEntry {
            original_size: 200,
            erasure_params: None,
            shards: HashMap::new(),
            updated_at: Utc::now(),
        };
        map_new.entries.insert("/x".to_string(), new_entry);
        map_new.version = 8;
        map_new.save_to_backend(backends[1].as_ref()).await.unwrap();

        let merged = ShardMap::load_from_all(&backends).await.unwrap();

        // Newer entry (size 200) should win
        assert_eq!(merged.get("/x").unwrap().original_size, 200);
        // Version should be max(5, 8) + 1 = 9
        assert_eq!(merged.version, 9);
    }

    // -- Deserialization error tests ------------------------------------------

    #[test]
    fn test_from_json_invalid_data() {
        let result = ShardMap::from_json(b"not valid json");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("deserialize"));
    }

    #[test]
    fn test_default_creates_empty_map() {
        let map = ShardMap::default();
        assert_eq!(map.version, 0);
        assert!(map.entries.is_empty());
    }

    // -- Tombstone / error-handling tests ------------------------------------

    #[tokio::test]
    async fn test_load_from_all_errors_when_all_fail() {
        // Store corrupted JSON at the shard map path so from_json fails.
        let backend: Arc<dyn StorageProvider> = Arc::new(MemoryProvider::new());
        let path = VaultPath::parse(SHARD_MAP_PATH).unwrap();
        let dir_path = VaultPath::parse(SHARD_MAP_DIR).unwrap();
        backend.create_dir(&dir_path).await.unwrap();
        backend
            .upload(&path, b"not valid json at all".to_vec())
            .await
            .unwrap();

        let backends: Vec<Arc<dyn StorageProvider>> = vec![backend];
        let result = ShardMap::load_from_all(&backends).await;
        assert!(
            result.is_err(),
            "Expected error when all backends return corrupt data"
        );
    }

    #[test]
    fn test_tombstone_prevents_resurrection() {
        // Map A removes entry X — tombstone is recorded.
        // Map B still has X (older timestamp). Merge B into A → X stays deleted.
        let backends = make_backends(2);
        let mut map_a = ShardMap::new();
        map_a.insert("/x", ShardMap::mirror_entry("/x", 100, &backends, None));
        map_a.remove("/x");
        assert!(map_a.get("/x").is_none());
        assert!(map_a.tombstones.contains_key("/x"));

        let mut map_b = ShardMap::new();
        // Give map_b's entry an older timestamp than A's tombstone.
        let old_ts = Utc::now() - chrono::Duration::seconds(60);
        let old_entry = ChunkEntry {
            original_size: 100,
            erasure_params: None,
            shards: HashMap::new(),
            updated_at: old_ts,
        };
        map_b.entries.insert("/x".to_string(), old_entry);

        map_a.merge(&map_b);

        assert!(
            map_a.get("/x").is_none(),
            "Deleted entry should not be resurrected by stale backend"
        );
    }

    #[test]
    fn test_tombstone_cleared_by_newer_entry() {
        // Map A tombstones X at T1.
        // Map B has X updated at T2 > T1.
        // Merge B into A → X is present (newer entry wins over tombstone).
        let backends = make_backends(2);
        let mut map_a = ShardMap::new();
        map_a.insert("/x", ShardMap::mirror_entry("/x", 100, &backends, None));
        map_a.remove("/x");
        assert!(map_a.tombstones.contains_key("/x"));

        let mut map_b = ShardMap::new();
        // Give map_b's entry a timestamp *after* A's tombstone.
        let future_ts = Utc::now() + chrono::Duration::seconds(60);
        let new_entry = ChunkEntry {
            original_size: 200,
            erasure_params: None,
            shards: HashMap::new(),
            updated_at: future_ts,
        };
        map_b.entries.insert("/x".to_string(), new_entry);

        map_a.merge(&map_b);

        assert!(
            map_a.get("/x").is_some(),
            "Entry newer than tombstone should be resurrected"
        );
        assert_eq!(map_a.get("/x").unwrap().original_size, 200);
    }
}
