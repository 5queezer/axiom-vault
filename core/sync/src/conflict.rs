//! Conflict detection and resolution.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use axiomvault_common::{Result, VaultPath};
use axiomvault_storage::{Metadata, StorageProvider};

use crate::state::SyncEntry;

/// Conflict resolution strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictStrategy {
    /// Keep both versions by renaming local.
    KeepBoth,
    /// Prefer local version, overwrite remote.
    PreferLocal,
    /// Prefer remote version, overwrite local.
    PreferRemote,
    /// Ask user to resolve manually.
    Manual,
}

/// Information about a detected conflict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictInfo {
    /// Path of the conflicted file.
    pub path: VaultPath,
    /// Local file metadata.
    pub local_etag: Option<String>,
    pub local_modified: DateTime<Utc>,
    pub local_size: Option<u64>,
    /// Remote file metadata.
    pub remote_etag: Option<String>,
    pub remote_modified: DateTime<Utc>,
    pub remote_size: Option<u64>,
    /// When the conflict was detected.
    pub detected_at: DateTime<Utc>,
}

impl ConflictInfo {
    /// Create conflict info from sync entry and remote metadata.
    pub fn from_entry_and_remote(entry: &SyncEntry, remote: &Metadata) -> Result<Self> {
        let path = VaultPath::parse(&entry.path)?;
        Ok(Self {
            path,
            local_etag: entry.local_etag.clone(),
            local_modified: entry.local_modified,
            local_size: None, // Will be populated by caller
            remote_etag: remote.etag.clone(),
            remote_modified: remote.modified,
            remote_size: remote.size,
            detected_at: Utc::now(),
        })
    }
}

/// Result of conflict resolution.
#[derive(Debug)]
pub enum ResolutionResult {
    /// Used local version.
    UsedLocal { new_remote_etag: Option<String> },
    /// Used remote version.
    UsedRemote { new_local_etag: Option<String> },
    /// Kept both versions, local renamed.
    KeptBoth {
        original_path: VaultPath,
        renamed_path: VaultPath,
        remote_etag: Option<String>,
    },
    /// Conflict still pending (manual resolution needed).
    Pending,
}

/// Conflict detector and resolver.
pub struct ConflictResolver {
    /// Default resolution strategy.
    default_strategy: ConflictStrategy,
}

impl ConflictResolver {
    /// Create a new conflict resolver with default strategy.
    pub fn new(default_strategy: ConflictStrategy) -> Self {
        Self { default_strategy }
    }

    /// Detect if there's a conflict between local and remote.
    pub fn detect_conflict(
        &self,
        local_etag: Option<&str>,
        remote_etag: Option<&str>,
        last_known_remote_etag: Option<&str>,
    ) -> bool {
        // No conflict if etags match
        if local_etag == remote_etag {
            return false;
        }

        // Conflict if remote has changed from what we last knew
        if let Some(last_known) = last_known_remote_etag {
            if remote_etag != Some(last_known) && local_etag != remote_etag {
                return true;
            }
        }

        // Conflict if local and remote both differ from each other
        // and we don't have a baseline
        local_etag.is_some() && remote_etag.is_some() && local_etag != remote_etag
    }

    /// Generate a conflict-renamed path (e.g., "file.txt" -> "file_conflict_20240115_123456.txt").
    pub fn generate_conflict_path(&self, original: &VaultPath) -> Result<VaultPath> {
        let original_str = original.to_string();
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S").to_string();

        let new_path = if let Some(dot_pos) = original_str.rfind('.') {
            let (name, ext) = original_str.split_at(dot_pos);
            format!("{}_conflict_{}{}", name, timestamp, ext)
        } else {
            format!("{}_conflict_{}", original_str, timestamp)
        };

        VaultPath::parse(&new_path)
    }

    /// Get the default resolution strategy.
    pub fn default_strategy(&self) -> ConflictStrategy {
        self.default_strategy
    }

    /// Resolve a conflict using the specified strategy.
    pub async fn resolve<P: StorageProvider + ?Sized>(
        &self,
        conflict: &ConflictInfo,
        local_data: Vec<u8>,
        provider: &P,
        strategy: ConflictStrategy,
    ) -> Result<ResolutionResult> {
        match strategy {
            ConflictStrategy::PreferLocal => {
                // Upload local version, overwriting remote
                let metadata = provider.upload(&conflict.path, local_data).await?;
                Ok(ResolutionResult::UsedLocal {
                    new_remote_etag: metadata.etag,
                })
            }
            ConflictStrategy::PreferRemote => {
                // Remote is already what we want, just return its etag
                Ok(ResolutionResult::UsedRemote {
                    new_local_etag: conflict.remote_etag.clone(),
                })
            }
            ConflictStrategy::KeepBoth => {
                // Rename local and upload as new file
                let renamed_path = self.generate_conflict_path(&conflict.path)?;
                let _metadata = provider.upload(&renamed_path, local_data).await?;

                // The remote version stays at original path
                Ok(ResolutionResult::KeptBoth {
                    original_path: conflict.path.clone(),
                    renamed_path,
                    remote_etag: conflict.remote_etag.clone(),
                })
            }
            ConflictStrategy::Manual => {
                // User must resolve
                Ok(ResolutionResult::Pending)
            }
        }
    }
}

impl Default for ConflictResolver {
    fn default() -> Self {
        Self::new(ConflictStrategy::KeepBoth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conflict_detection_no_conflict() {
        let resolver = ConflictResolver::default();

        // Same etag = no conflict
        assert!(!resolver.detect_conflict(Some("etag1"), Some("etag1"), Some("etag1")));

        // Both none = no conflict
        assert!(!resolver.detect_conflict(None, None, None));
    }

    #[test]
    fn test_conflict_detection_with_conflict() {
        let resolver = ConflictResolver::default();

        // Different etags with known baseline
        assert!(resolver.detect_conflict(
            Some("local_etag"),
            Some("remote_etag"),
            Some("old_etag")
        ));

        // Different etags without baseline
        assert!(resolver.detect_conflict(Some("local_etag"), Some("remote_etag"), None));
    }

    #[test]
    fn test_generate_conflict_path_with_extension() {
        let resolver = ConflictResolver::default();
        let original = VaultPath::parse("/docs/report.pdf").unwrap();
        let conflict_path = resolver.generate_conflict_path(&original).unwrap();

        let path_str = conflict_path.to_string();
        assert!(path_str.contains("_conflict_"));
        assert!(path_str.ends_with(".pdf"));
        assert!(path_str.starts_with("/docs/report_conflict_"));
    }

    #[test]
    fn test_generate_conflict_path_without_extension() {
        let resolver = ConflictResolver::default();
        let original = VaultPath::parse("/docs/README").unwrap();
        let conflict_path = resolver.generate_conflict_path(&original).unwrap();

        let path_str = conflict_path.to_string();
        assert!(path_str.contains("_conflict_"));
        assert!(path_str.starts_with("/docs/README_conflict_"));
    }
}
