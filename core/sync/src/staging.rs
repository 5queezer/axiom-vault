//! Local staging area for atomic writes.
//!
//! # Layering contract — MUST READ
//!
//! The bytes passed to [`StagingArea::stage_upload`] **MUST be ciphertext**.
//! This layer is *not* encrypted at rest: it holds whatever the caller
//! hands it on the local filesystem, with `0600` (file) and `0700`
//! (directory) permissions on Unix as defense-in-depth, but those modes
//! do not protect against a same-user attacker (compromised process,
//! malicious app on a phone) reading the staging tree directly.
//!
//! Concretely: the vault MUST encrypt before staging; `stage_upload` is
//! not allowed to see plaintext. See audit finding M-5 in
//! `docs/SECURITY_AUDIT_2026-04-21.md`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::warn;
use uuid::Uuid;

use axiomvault_common::{Error, Result, VaultPath};

/// Open `path` for writing with `0o600` permissions on Unix, fail if it
/// already exists. On non-Unix this falls back to a plain create-new write.
///
/// We use `create_new(true)` so a stale file with looser permissions cannot
/// be reused — the open will fail and the caller can recover.
async fn write_private_file(path: &Path, data: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        // Note: `tokio::fs::OpenOptions::mode` is inherent on Unix — no
        // `std::os::unix::fs::OpenOptionsExt` import required.
        use tokio::io::AsyncWriteExt;

        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .await?;
        file.write_all(data).await?;
        file.flush().await?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await?;
        file.write_all(data).await?;
        file.flush().await?;
        Ok(())
    }
}

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
    ///
    /// On Unix the staging directory is chmod'd to `0o700` so other local
    /// users cannot enumerate or read pending changes (audit M-5).
    ///
    /// If the on-disk registry exists but is corrupt (invalid JSON), the
    /// corrupt file is renamed to `staging_registry.json.corrupt-{ts}` so
    /// the operator can inspect it later, and a fresh empty registry is
    /// started instead of silently dropping all in-flight changes (audit
    /// L-7).
    pub async fn new(base_dir: impl AsRef<Path>) -> Result<Self> {
        let base_dir = base_dir.as_ref().to_path_buf();
        let staging_dir = base_dir.join("staging");
        let registry_path = base_dir.join("staging_registry.json");

        // Create staging directory.
        fs::create_dir_all(&staging_dir).await.map_err(Error::Io)?;

        // Tighten directory permissions on Unix (audit M-5).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o700);
            fs::set_permissions(&staging_dir, perms)
                .await
                .map_err(Error::Io)?;
        }

        // Load existing registry if present. Corrupt JSON is preserved on
        // disk (renamed) so it can be inspected, and we start fresh —
        // never silently drop in-flight changes (audit L-7).
        let changes = if registry_path.exists() {
            let content = fs::read_to_string(&registry_path)
                .await
                .map_err(Error::Io)?;
            match serde_json::from_str::<HashMap<String, StagedChange>>(&content) {
                Ok(map) => map,
                Err(e) => {
                    // chrono's `%6f` is a width directive for nanoseconds and
                    // does not emit fractional seconds on its own; build the
                    // microsecond suffix explicitly so renames within the same
                    // second still differ (audit M-6).
                    let now = Utc::now();
                    let ts = format!(
                        "{}_{:06}",
                        now.format("%Y%m%d_%H%M%S"),
                        now.timestamp_subsec_micros()
                    );
                    let corrupt_path = registry_path
                        .with_file_name(format!("staging_registry.json.corrupt-{}", ts));
                    warn!(
                        "staging registry at {} is corrupt ({}); preserving as {} and starting a fresh registry (audit L-7)",
                        registry_path.display(),
                        e,
                        corrupt_path.display()
                    );
                    if let Err(rename_err) = fs::rename(&registry_path, &corrupt_path).await {
                        warn!(
                            "failed to rename corrupt staging registry {} -> {}: {}",
                            registry_path.display(),
                            corrupt_path.display(),
                            rename_err
                        );
                    }
                    HashMap::new()
                }
            }
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
    ///
    /// **Contract:** `data` MUST be ciphertext. See the module-level docs
    /// (audit M-5).
    pub async fn stage_upload(
        &mut self,
        vault_path: &VaultPath,
        data: Vec<u8>,
        change_type: ChangeType,
    ) -> Result<String> {
        let change_id = Uuid::new_v4().to_string();
        let staging_file = self.base_dir.join(&change_id);

        // Write data to staging file with mode 0o600 on Unix (audit M-5).
        write_private_file(&staging_file, &data)
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
                fs::remove_file(staging_file).await.map_err(Error::Io)?;
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

    /// Persist the registry to disk atomically (write-to-temp + rename).
    ///
    /// On Unix the temp file is created with mode `0o600` so the registry
    /// (which contains pending change metadata: paths, sizes, change types)
    /// is not world-readable (audit M-5). A leftover temp file from a
    /// previous crashed write is removed first so `create_new` succeeds.
    async fn persist_registry(&self) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.changes)
            .map_err(|e| Error::Serialization(e.to_string()))?;

        let tmp_path = self.registry_path.with_extension("json.tmp");
        if tmp_path.exists() {
            // Best-effort cleanup of a stale temp from a prior crashed write.
            if let Err(e) = fs::remove_file(&tmp_path).await {
                warn!(
                    "failed to remove stale staging registry temp {}: {}",
                    tmp_path.display(),
                    e
                );
            }
        }
        write_private_file(&tmp_path, json.as_bytes())
            .await
            .map_err(Error::Io)?;
        fs::rename(&tmp_path, &self.registry_path)
            .await
            .map_err(Error::Io)
    }

    /// Clean up orphaned staging files.
    pub async fn cleanup_orphaned(&mut self) -> Result<usize> {
        let mut cleaned = 0;
        let mut entries = fs::read_dir(&self.base_dir).await.map_err(Error::Io)?;

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

    /// Audit M-5: staged files (and the registry) must be `0o600` on Unix
    /// so other local users cannot read pending ciphertext or metadata.
    #[cfg(unix)]
    #[tokio::test]
    async fn test_staged_file_is_mode_0600_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let mut staging = StagingArea::new(temp.path()).await.unwrap();

        let path = VaultPath::parse("/secret.bin").unwrap();
        let change_id = staging
            .stage_upload(&path, b"ciphertext".to_vec(), ChangeType::Create)
            .await
            .unwrap();

        // The staging file lives under <temp>/staging/<change_id>.
        let staged_file = temp.path().join("staging").join(&change_id);
        let file_meta = std::fs::metadata(&staged_file).expect("staged file exists");
        let file_mode = file_meta.permissions().mode() & 0o777;
        assert_eq!(
            file_mode, 0o600,
            "staged file mode is {:o}, want 0o600",
            file_mode
        );

        // The registry file should also be 0o600 (it contains pending
        // change metadata).
        let registry_file = temp.path().join("staging_registry.json");
        let reg_meta = std::fs::metadata(&registry_file).expect("registry file exists");
        let reg_mode = reg_meta.permissions().mode() & 0o777;
        assert_eq!(
            reg_mode, 0o600,
            "registry file mode is {:o}, want 0o600",
            reg_mode
        );

        // The staging directory itself should be 0o700.
        let dir_meta = std::fs::metadata(temp.path().join("staging")).expect("staging dir exists");
        let dir_mode = dir_meta.permissions().mode() & 0o777;
        assert_eq!(
            dir_mode, 0o700,
            "staging dir mode is {:o}, want 0o700",
            dir_mode
        );
    }

    /// Audit L-7: when the on-disk registry is corrupt JSON, `StagingArea::new`
    /// must rename the corrupt file aside (so the operator can inspect it)
    /// and start with a fresh empty registry — never silently drop the bad
    /// file.
    #[tokio::test]
    async fn test_corrupt_registry_is_renamed_and_fresh_starts() {
        let temp = TempDir::new().unwrap();
        let registry_path = temp.path().join("staging_registry.json");

        // Plant a corrupt registry file before construction.
        tokio::fs::write(&registry_path, b"{ this is not valid json")
            .await
            .unwrap();

        let staging = StagingArea::new(temp.path()).await.unwrap();
        // Fresh empty registry.
        assert_eq!(staging.count(), 0);
        // The corrupt file must NOT still be at the original path with the
        // bad bytes — it must have been renamed aside (or, if rename
        // failed, the warn was logged; we don't assert that here).
        if registry_path.exists() {
            // If a fresh registry has been written since (e.g. by a later
            // persist_registry call), it must now be valid JSON. We
            // haven't done any mutating ops, so the file should not exist
            // at all OR should be valid JSON.
            let content = tokio::fs::read_to_string(&registry_path).await.unwrap();
            assert!(
                serde_json::from_str::<HashMap<String, StagedChange>>(&content).is_ok(),
                "registry at original path must be valid JSON or absent"
            );
        }

        // A `staging_registry.json.corrupt-*` sibling should exist.
        let mut found_corrupt_sibling = false;
        let mut entries = tokio::fs::read_dir(temp.path()).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("staging_registry.json.corrupt-") {
                found_corrupt_sibling = true;
                let content = tokio::fs::read(entry.path()).await.unwrap();
                assert_eq!(
                    content, b"{ this is not valid json",
                    "corrupt sibling should preserve original bytes"
                );
                break;
            }
        }
        assert!(
            found_corrupt_sibling,
            "expected a `staging_registry.json.corrupt-*` sibling preserving the bad file"
        );
    }

    /// Audit M-6 regression lock: the `corrupt-*` suffix on the renamed
    /// registry must include a 6-digit microsecond segment, not the 0-length
    /// segment chrono's misused `%6f` width directive produced.
    /// Shape: `staging_registry.json.corrupt-\d{8}_\d{6}_\d{6}`.
    #[tokio::test]
    async fn test_corrupt_registry_rename_suffix_has_microseconds() {
        let temp = TempDir::new().unwrap();
        let registry_path = temp.path().join("staging_registry.json");
        tokio::fs::write(&registry_path, b"{ broken").await.unwrap();

        let _staging = StagingArea::new(temp.path()).await.unwrap();

        let mut suffix: Option<String> = None;
        let mut entries = tokio::fs::read_dir(temp.path()).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy().to_string();
            if let Some(s) = name_str.strip_prefix("staging_registry.json.corrupt-") {
                suffix = Some(s.to_string());
                break;
            }
        }
        let suffix = suffix.expect("corrupt sibling must exist");

        // Must be exactly YYYYMMDD_HHMMSS_uuuuuu = 8+1+6+1+6 = 22 chars.
        assert_eq!(
            suffix.len(),
            22,
            "corrupt suffix `{}` has unexpected length (chrono %6f bug?)",
            suffix
        );
        let parts: Vec<&str> = suffix.split('_').collect();
        assert_eq!(
            parts.len(),
            3,
            "suffix must be 3 underscore-separated parts"
        );
        assert_eq!(parts[0].len(), 8);
        assert!(parts[0].chars().all(|c| c.is_ascii_digit()));
        assert_eq!(parts[1].len(), 6);
        assert!(parts[1].chars().all(|c| c.is_ascii_digit()));
        // The microsecond segment was 0-length under the %6f bug — assert
        // it is the full 6 digits.
        assert_eq!(
            parts[2].len(),
            6,
            "microsecond segment has wrong length (chrono %6f bug?)"
        );
        assert!(parts[2].chars().all(|c| c.is_ascii_digit()));
    }
}
