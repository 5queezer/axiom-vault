//! Local staging area for atomic writes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use uuid::Uuid;

use axiomvault_common::{Error, Result, VaultPath};

/// A staged change waiting to be committed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagedChange {
    /// Unique ID for this change.
    pub id: String,
    /// Vault path this change applies to.
    pub vault_path: VaultPath,
    /// Type of change.
    pub change_type: ChangeType,
    /// When the change was staged.
    pub staged_at: DateTime<Utc>,
    /// Local file path to the staged content (for uploads).
    pub staging_file: Option<PathBuf>,
    /// Size of the data.
    pub size: u64,
}

/// Type of staged change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeType {
    /// New file to upload.
    Create,
    /// Existing file modified.
    Update,
    /// File to delete.
    Delete,
}

/// Local staging area for managing pending changes.
pub struct StagingArea {
    /// Base directory for staging files.
    base_dir: PathBuf,
    /// In-memory registry of staged changes.
    changes: HashMap<String, StagedChange>,
    /// Path to persist the registry.
    registry_path: PathBuf,
}

impl StagingArea {
    /// Create a new staging area.
    pub async fn new(base_dir: impl AsRef<Path>) -> Result<Self> {
        let base_dir = base_dir.as_ref().to_path_buf();
        let staging_dir = base_dir.join("staging");
        let registry_path = base_dir.join("staging_registry.json");

        // Create staging directory
        fs::create_dir_all(&staging_dir)
            .await
            .map_err(Error::Io)?;

        // Load existing registry if present
        let changes = if registry_path.exists() {
            let content = fs::read_to_string(&registry_path)
                .await
                .map_err(Error::Io)?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            HashMap::new()
        };

        Ok(Self {
            base_dir: staging_dir,
            changes,
            registry_path,
        })
    }

    /// Stage data for upload.
    pub async fn stage_upload(
        &mut self,
        vault_path: &VaultPath,
        data: Vec<u8>,
        change_type: ChangeType,
    ) -> Result<String> {
        let change_id = Uuid::new_v4().to_string();
        let staging_file = self.base_dir.join(&change_id);

        // Write data to staging file
        fs::write(&staging_file, &data)
            .await
            .map_err(Error::Io)?;

        let change = StagedChange {
            id: change_id.clone(),
            vault_path: vault_path.clone(),
            change_type,
            staged_at: Utc::now(),
            staging_file: Some(staging_file),
            size: data.len() as u64,
        };

        self.changes.insert(change_id.clone(), change);
        self.persist_registry().await?;

        Ok(change_id)
    }

    /// Stage a delete operation.
    pub async fn stage_delete(&mut self, vault_path: &VaultPath) -> Result<String> {
        let change_id = Uuid::new_v4().to_string();

        let change = StagedChange {
            id: change_id.clone(),
            vault_path: vault_path.clone(),
            change_type: ChangeType::Delete,
            staged_at: Utc::now(),
            staging_file: None,
            size: 0,
        };

        self.changes.insert(change_id.clone(), change);
        self.persist_registry().await?;

        Ok(change_id)
    }

    /// Get staged data by change ID.
    pub async fn get_staged_data(&self, change_id: &str) -> Result<Vec<u8>> {
        let change = self
            .changes
            .get(change_id)
            .ok_or_else(|| Error::NotFound(format!("Staged change not found: {}", change_id)))?;

        let staging_file = change.staging_file.as_ref().ok_or_else(|| {
            Error::InvalidInput("No staging file for this change type".to_string())
        })?;

        fs::read(staging_file).await.map_err(Error::Io)
    }

    /// Get a staged change by ID.
    pub fn get_change(&self, change_id: &str) -> Option<&StagedChange> {
        self.changes.get(change_id)
    }

    /// Get all staged changes.
    pub fn all_changes(&self) -> impl Iterator<Item = &StagedChange> {
        self.changes.values()
    }

    /// Get changes for a specific vault path.
    pub fn changes_for_path(&self, vault_path: &VaultPath) -> Vec<&StagedChange> {
        self.changes
            .values()
            .filter(|c| &c.vault_path == vault_path)
            .collect()
    }

    /// Commit (remove) a staged change after successful sync.
    pub async fn commit(&mut self, change_id: &str) -> Result<()> {
        let change = self
            .changes
            .remove(change_id)
            .ok_or_else(|| Error::NotFound(format!("Staged change not found: {}", change_id)))?;

        // Delete the staging file if present
        if let Some(staging_file) = &change.staging_file {
            if staging_file.exists() {
                fs::remove_file(staging_file)
                    .await
                    .map_err(Error::Io)?;
            }
        }

        self.persist_registry().await?;
        Ok(())
    }

    /// Rollback (remove) a staged change without committing.
    pub async fn rollback(&mut self, change_id: &str) -> Result<()> {
        // Same as commit - just removes from staging
        self.commit(change_id).await
    }

    /// Clear all staged changes.
    pub async fn clear(&mut self) -> Result<()> {
        let change_ids: Vec<String> = self.changes.keys().cloned().collect();
        for change_id in change_ids {
            self.commit(&change_id).await?;
        }
        Ok(())
    }

    /// Get total size of staged data.
    pub fn total_size(&self) -> u64 {
        self.changes.values().map(|c| c.size).sum()
    }

    /// Get count of staged changes.
    pub fn count(&self) -> usize {
        self.changes.len()
    }

    /// Check if staging area is empty.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Persist the registry to disk.
    async fn persist_registry(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.changes)
            .map_err(|e| Error::Serialization(e.to_string()))?;
        fs::write(&self.registry_path, json)
            .await
            .map_err(Error::Io)
    }

    /// Clean up orphaned staging files.
    pub async fn cleanup_orphaned(&mut self) -> Result<usize> {
        let mut cleaned = 0;
        let mut entries = fs::read_dir(&self.base_dir)
            .await
            .map_err(Error::Io)?;

        let known_files: std::collections::HashSet<PathBuf> = self
            .changes
            .values()
            .filter_map(|c| c.staging_file.clone())
            .collect();

        while let Some(entry) = entries.next_entry().await.map_err(Error::Io)? {
            let path = entry.path();
            if path.is_file() && !known_files.contains(&path) {
                fs::remove_file(&path).await.map_err(Error::Io)?;
                cleaned += 1;
            }
        }

        Ok(cleaned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_staging_area_creation() {
        let temp = TempDir::new().unwrap();
        let staging = StagingArea::new(temp.path()).await.unwrap();
        assert!(staging.is_empty());
    }

    #[tokio::test]
    async fn test_stage_upload() {
        let temp = TempDir::new().unwrap();
        let mut staging = StagingArea::new(temp.path()).await.unwrap();

        let path = VaultPath::parse("/test.txt").unwrap();
        let data = b"Hello, World!".to_vec();

        let change_id = staging
            .stage_upload(&path, data.clone(), ChangeType::Create)
            .await
            .unwrap();

        assert!(!staging.is_empty());
        assert_eq!(staging.count(), 1);

        let retrieved = staging.get_staged_data(&change_id).await.unwrap();
        assert_eq!(retrieved, data);
    }

    #[tokio::test]
    async fn test_stage_delete() {
        let temp = TempDir::new().unwrap();
        let mut staging = StagingArea::new(temp.path()).await.unwrap();

        let path = VaultPath::parse("/test.txt").unwrap();
        let _change_id = staging.stage_delete(&path).await.unwrap();

        let changes: Vec<_> = staging.changes_for_path(&path);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_type, ChangeType::Delete);
    }

    #[tokio::test]
    async fn test_commit_change() {
        let temp = TempDir::new().unwrap();
        let mut staging = StagingArea::new(temp.path()).await.unwrap();

        let path = VaultPath::parse("/test.txt").unwrap();
        let change_id = staging
            .stage_upload(&path, b"data".to_vec(), ChangeType::Create)
            .await
            .unwrap();

        assert_eq!(staging.count(), 1);

        staging.commit(&change_id).await.unwrap();
        assert!(staging.is_empty());
    }

    #[tokio::test]
    async fn test_persistence() {
        let temp = TempDir::new().unwrap();

        // Create and stage
        {
            let mut staging = StagingArea::new(temp.path()).await.unwrap();
            let path = VaultPath::parse("/test.txt").unwrap();
            staging
                .stage_upload(&path, b"data".to_vec(), ChangeType::Create)
                .await
                .unwrap();
        }

        // Reload and verify
        {
            let staging = StagingArea::new(temp.path()).await.unwrap();
            assert_eq!(staging.count(), 1);
        }
    }
}
