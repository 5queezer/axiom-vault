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
    ///
    /// A conflict exists only when *both* sides have changed since the last
    /// known synchronised state.  If only the local side changed (remote etag
    /// still matches the last-known value) we can safely push; if only the
    /// remote side changed we can safely pull.
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

        // If we have a baseline, conflict only when the remote diverged from
        // what we last knew *and* the local side also diverged.
        if let Some(last_known) = last_known_remote_etag {
            let remote_changed = remote_etag != Some(last_known);
            let local_changed = local_etag != Some(last_known);
            return remote_changed && local_changed;
        }

        // Without a baseline we cannot tell who changed; treat as conflict
        // only when both sides have an etag and they differ.
        local_etag.is_some() && remote_etag.is_some() && local_etag != remote_etag
    }

    /// Generate a conflict-renamed path
    /// (e.g., "file.txt" -> "file_conflict_20240115_123456_123456_a1b2.txt").
    ///
    /// The suffix combines a microsecond-resolution timestamp and a 4-char
    /// random hex tag so two conflicts on the same path within the same
    /// wall-clock second cannot collide (audit M-6,
    /// SECURITY_AUDIT_2026-04-21.md).
    pub fn generate_conflict_path(&self, original: &VaultPath) -> Result<VaultPath> {
        use rand::RngExt;

        let original_str = original.to_string();
        // %6f -> microseconds (6-digit fractional seconds).
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S_%6f").to_string();

        // 16 random bits rendered as 4 lowercase hex chars.
        let mut rand_bytes = [0u8; 2];
        rand::rng().fill(&mut rand_bytes[..]);
        let rand_suffix = format!("{:02x}{:02x}", rand_bytes[0], rand_bytes[1]);

        let new_path = if let Some(dot_pos) = original_str.rfind('.') {
            let (name, ext) = original_str.split_at(dot_pos);
            format!("{}_conflict_{}_{}{}", name, timestamp, rand_suffix, ext)
        } else {
            format!("{}_conflict_{}_{}", original_str, timestamp, rand_suffix)
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

    /// Audit M-6: conflict paths must include a sub-second component and a
    /// random suffix so same-second collisions are astronomically unlikely.
    /// Format we expect (for `/docs/report.pdf`):
    ///   /docs/report_conflict_YYYYMMDD_HHMMSS_uuuuuu_xxxx.pdf
    ///   - YYYYMMDD     : 8 digits
    ///   - HHMMSS       : 6 digits
    ///   - uuuuuu       : 6 digits microseconds
    ///   - xxxx         : 4 lowercase hex chars
    #[test]
    fn test_generate_conflict_path_format_includes_random_suffix() {
        let resolver = ConflictResolver::default();
        let original = VaultPath::parse("/docs/report.pdf").unwrap();
        let conflict_path = resolver.generate_conflict_path(&original).unwrap();
        let path_str = conflict_path.to_string();

        // Strip the deterministic prefix and suffix.
        let stem = path_str
            .strip_prefix("/docs/report_conflict_")
            .expect("conflict path should start with the original stem prefix");
        let stem = stem
            .strip_suffix(".pdf")
            .expect("conflict path should preserve the original extension");

        // Now `stem` should look like: 20240115_123456_123456_a1b2
        // i.e. 8 + 1 + 6 + 1 + 6 + 1 + 4 = 27 chars.
        assert_eq!(
            stem.len(),
            27,
            "conflict path stem `{}` has unexpected length",
            stem
        );

        let parts: Vec<&str> = stem.split('_').collect();
        assert_eq!(
            parts.len(),
            4,
            "conflict path stem `{}` must have 4 underscore-separated parts",
            stem
        );

        // Date YYYYMMDD
        assert_eq!(parts[0].len(), 8);
        assert!(parts[0].chars().all(|c| c.is_ascii_digit()));
        // Time HHMMSS
        assert_eq!(parts[1].len(), 6);
        assert!(parts[1].chars().all(|c| c.is_ascii_digit()));
        // Microseconds (6 digits)
        assert_eq!(parts[2].len(), 6);
        assert!(parts[2].chars().all(|c| c.is_ascii_digit()));
        // Random hex suffix (4 lowercase hex chars)
        assert_eq!(parts[3].len(), 4);
        assert!(parts[3]
            .chars()
            .all(|c| c.is_ascii_hexdigit() && (c.is_ascii_digit() || c.is_ascii_lowercase())));
    }

    /// Audit M-6: two conflict paths generated back-to-back must differ even
    /// when the wall-clock second is identical (the random suffix and the
    /// microseconds together make a collision astronomically unlikely).
    #[test]
    fn test_generate_conflict_path_unique_within_same_second() {
        let resolver = ConflictResolver::default();
        let original = VaultPath::parse("/file.txt").unwrap();

        let mut seen = std::collections::HashSet::new();
        for _ in 0..32 {
            let p = resolver.generate_conflict_path(&original).unwrap();
            let inserted = seen.insert(p.to_string());
            assert!(inserted, "duplicate conflict path generated back-to-back");
        }
    }
}
