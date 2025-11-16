//! Sync state tracking and persistence.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use axiomvault_common::{Result, Error, VaultPath};

/// Sync status for a single file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SyncStatus {
    /// File is in sync with remote.
    Synced,
    /// Local changes pending upload.
    LocalModified,
    /// Remote changes pending download.
    RemoteModified,
    /// Both local and remote have changes (conflict).
    Conflicted,
    /// File is being synced.
    Syncing,
    /// Sync failed.
    Failed,
}

/// Metadata for tracking sync state of a single item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncEntry {
    /// Path in the vault.
    pub path: String,
    /// Local etag/revision.
    pub local_etag: Option<String>,
    /// Remote etag/revision (last known).
    pub remote_etag: Option<String>,
    /// Local modification time.
    pub local_modified: DateTime<Utc>,
    /// Remote modification time (last known).
    pub remote_modified: Option<DateTime<Utc>>,
    /// Current sync status.
    pub status: SyncStatus,
    /// Last successful sync time.
    pub last_synced: Option<DateTime<Utc>>,
    /// Number of failed sync attempts.
    pub failure_count: u32,
    /// Last error message if failed.
    pub last_error: Option<String>,
}

impl SyncEntry {
    /// Create a new sync entry for a local file.
    pub fn new_local(path: impl Into<String>, local_etag: Option<String>) -> Self {
        Self {
            path: path.into(),
            local_etag,
            remote_etag: None,
            local_modified: Utc::now(),
            remote_modified: None,
            status: SyncStatus::LocalModified,
            last_synced: None,
            failure_count: 0,
            last_error: None,
        }
    }

    /// Create a sync entry for a synced file.
    pub fn new_synced(
        path: impl Into<String>,
        etag: Option<String>,
        modified: DateTime<Utc>,
    ) -> Self {
        Self {
            path: path.into(),
            local_etag: etag.clone(),
            remote_etag: etag,
            local_modified: modified,
            remote_modified: Some(modified),
            status: SyncStatus::Synced,
            last_synced: Some(Utc::now()),
            failure_count: 0,
            last_error: None,
        }
    }

    /// Mark as syncing.
    pub fn mark_syncing(&mut self) {
        self.status = SyncStatus::Syncing;
    }

    /// Mark as synced successfully.
    pub fn mark_synced(&mut self, etag: Option<String>, modified: DateTime<Utc>) {
        self.local_etag = etag.clone();
        self.remote_etag = etag;
        self.local_modified = modified;
        self.remote_modified = Some(modified);
        self.status = SyncStatus::Synced;
        self.last_synced = Some(Utc::now());
        self.failure_count = 0;
        self.last_error = None;
    }

    /// Mark as failed.
    pub fn mark_failed(&mut self, error: impl Into<String>) {
        self.status = SyncStatus::Failed;
        self.failure_count += 1;
        self.last_error = Some(error.into());
    }

    /// Mark as conflicted.
    pub fn mark_conflicted(&mut self, remote_etag: Option<String>, remote_modified: DateTime<Utc>) {
        self.remote_etag = remote_etag;
        self.remote_modified = Some(remote_modified);
        self.status = SyncStatus::Conflicted;
    }

    /// Mark local as modified.
    pub fn mark_local_modified(&mut self, etag: Option<String>) {
        self.local_etag = etag;
        self.local_modified = Utc::now();
        if self.status == SyncStatus::Synced {
            self.status = SyncStatus::LocalModified;
        }
    }

    /// Mark remote as modified.
    pub fn mark_remote_modified(&mut self, etag: Option<String>, modified: DateTime<Utc>) {
        if self.remote_etag != etag {
            self.remote_etag = etag;
            self.remote_modified = Some(modified);
            if self.status == SyncStatus::Synced {
                self.status = SyncStatus::RemoteModified;
            } else if self.status == SyncStatus::LocalModified {
                self.status = SyncStatus::Conflicted;
            }
        }
    }

    /// Check if sync should be retried.
    pub fn should_retry(&self, max_retries: u32) -> bool {
        self.status == SyncStatus::Failed && self.failure_count < max_retries
    }
}

/// Overall sync state for the vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncState {
    /// Sync entries by path.
    entries: HashMap<String, SyncEntry>,
    /// Last full sync time.
    pub last_full_sync: Option<DateTime<Utc>>,
    /// Whether a sync is currently in progress.
    pub sync_in_progress: bool,
}

impl SyncState {
    /// Create a new empty sync state.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            last_full_sync: None,
            sync_in_progress: false,
        }
    }

    /// Get sync entry for a path.
    pub fn get(&self, path: &VaultPath) -> Option<&SyncEntry> {
        self.entries.get(&path.to_string())
    }

    /// Get mutable sync entry for a path.
    pub fn get_mut(&mut self, path: &VaultPath) -> Option<&mut SyncEntry> {
        self.entries.get_mut(&path.to_string())
    }

    /// Insert or update a sync entry.
    pub fn insert(&mut self, entry: SyncEntry) {
        self.entries.insert(entry.path.clone(), entry);
    }

    /// Remove a sync entry.
    pub fn remove(&mut self, path: &VaultPath) -> Option<SyncEntry> {
        self.entries.remove(&path.to_string())
    }

    /// Get all entries.
    pub fn entries(&self) -> impl Iterator<Item = &SyncEntry> {
        self.entries.values()
    }

    /// Get all mutable entries.
    pub fn entries_mut(&mut self) -> impl Iterator<Item = &mut SyncEntry> {
        self.entries.values_mut()
    }

    /// Get entries with a specific status.
    pub fn entries_with_status(&self, status: SyncStatus) -> Vec<&SyncEntry> {
        self.entries.values().filter(|e| e.status == status).collect()
    }

    /// Get all paths.
    pub fn paths(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    /// Count entries by status.
    pub fn count_by_status(&self) -> HashMap<SyncStatus, usize> {
        let mut counts = HashMap::new();
        for entry in self.entries.values() {
            *counts.entry(entry.status).or_insert(0) += 1;
        }
        counts
    }

    /// Check if there are pending changes.
    pub fn has_pending_changes(&self) -> bool {
        self.entries.values().any(|e| {
            matches!(
                e.status,
                SyncStatus::LocalModified | SyncStatus::RemoteModified | SyncStatus::Conflicted
            )
        })
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| Error::Serialization(e.to_string()))
    }

    /// Deserialize from JSON.
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|e| Error::Serialization(e.to_string()))
    }
}

impl Default for SyncState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_entry_creation() {
        let entry = SyncEntry::new_local("/test.txt", Some("etag123".to_string()));
        assert_eq!(entry.status, SyncStatus::LocalModified);
        assert_eq!(entry.local_etag, Some("etag123".to_string()));
        assert!(entry.remote_etag.is_none());
    }

    #[test]
    fn test_mark_synced() {
        let mut entry = SyncEntry::new_local("/test.txt", Some("etag1".to_string()));
        let now = Utc::now();
        entry.mark_synced(Some("etag2".to_string()), now);

        assert_eq!(entry.status, SyncStatus::Synced);
        assert_eq!(entry.local_etag, Some("etag2".to_string()));
        assert_eq!(entry.remote_etag, Some("etag2".to_string()));
        assert!(entry.last_synced.is_some());
    }

    #[test]
    fn test_conflict_detection() {
        let mut entry = SyncEntry::new_synced("/test.txt", Some("etag1".to_string()), Utc::now());

        // Local modification
        entry.mark_local_modified(Some("etag2".to_string()));
        assert_eq!(entry.status, SyncStatus::LocalModified);

        // Remote modification while local is modified -> conflict
        entry.mark_remote_modified(Some("etag3".to_string()), Utc::now());
        assert_eq!(entry.status, SyncStatus::Conflicted);
    }

    #[test]
    fn test_sync_state() {
        let mut state = SyncState::new();

        let entry1 = SyncEntry::new_local("/file1.txt", Some("e1".to_string()));
        let entry2 = SyncEntry::new_synced("/file2.txt", Some("e2".to_string()), Utc::now());

        state.insert(entry1);
        state.insert(entry2);

        assert_eq!(state.entries().count(), 2);
        assert!(state.has_pending_changes());

        let counts = state.count_by_status();
        assert_eq!(*counts.get(&SyncStatus::LocalModified).unwrap_or(&0), 1);
        assert_eq!(*counts.get(&SyncStatus::Synced).unwrap_or(&0), 1);
    }

    #[test]
    fn test_state_serialization() {
        let mut state = SyncState::new();
        state.insert(SyncEntry::new_local("/test.txt", Some("etag".to_string())));

        let json = state.to_json().unwrap();
        let restored = SyncState::from_json(&json).unwrap();

        assert_eq!(restored.entries().count(), 1);
    }
}
