//! SQLite-based local index for vault metadata caching.
//!
//! Persists vault tree state locally for faster startup and offline access.
//!
//! **Security note:** This index caches plaintext vault metadata (filenames,
//! directory structure, sizes, timestamps) outside the encrypted vault. The
//! database is only populated while the vault is unlocked, and MUST be cleared
//! when the vault is locked to avoid leaking sensitive metadata at rest.
//! Database files are created with restrictive permissions (0600) to limit
//! access to the owning user.

use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use tracing::{debug, info};

use crate::error::{AppError, AppResult};

fn sqlite_err(e: rusqlite::Error) -> AppError {
    AppError::Storage(format!("SQLite error: {}", e))
}

fn lock_err() -> AppError {
    AppError::Internal("Failed to acquire index lock".to_string())
}

/// Represents a cached vault entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub path: String,
    pub encrypted_name: String,
    pub is_directory: bool,
    pub size: Option<i64>,
    pub modified_at: i64,
    pub etag: Option<String>,
}

/// Local index manager using SQLite.
pub struct LocalIndex {
    conn: Mutex<Connection>,
}

impl LocalIndex {
    /// Create or open a local index database.
    ///
    /// The database file is created with mode 0600 (owner read/write only)
    /// to prevent other users from reading cached vault metadata.
    pub fn open(db_path: impl AsRef<Path>) -> AppResult<Self> {
        let db_path = db_path.as_ref();

        #[cfg(not(unix))]
        if db_path.to_str() != Some(":memory:") {
            return Err(AppError::Storage(
                "secure local-index ACL creation is not implemented on this platform".to_string(),
            ));
        }

        #[cfg(unix)]
        if db_path.to_str() != Some(":memory:") {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

            // `mode` applies at the atomic create boundary, so the database is
            // never observable with broader permissions even before repair.
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(db_path)
                .map_err(|e| AppError::Storage(format!("Failed to securely open index db: {e}")))?;
            drop(file);

            std::fs::set_permissions(db_path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| AppError::Storage(format!("Failed to secure index db: {e}")))?;
            let mode = std::fs::metadata(db_path)
                .map_err(|e| AppError::Storage(format!("Failed to verify index db: {e}")))?
                .permissions()
                .mode()
                & 0o777;
            if mode != 0o600 {
                return Err(AppError::Storage(format!(
                    "Unsafe index db permissions after repair: {mode:o}"
                )));
            }
        }

        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(sqlite_err)?;

        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS vault_entries (
                path TEXT PRIMARY KEY,
                encrypted_name TEXT NOT NULL,
                is_directory INTEGER NOT NULL,
                size INTEGER,
                modified_at INTEGER NOT NULL,
                etag TEXT
            );

            CREATE TABLE IF NOT EXISTS vault_metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_parent ON vault_entries(path);
            "#,
        )
        .map_err(sqlite_err)?;

        info!("Local index opened successfully");
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Create an in-memory index (for testing).
    pub fn in_memory() -> AppResult<Self> {
        Self::open(":memory:")
    }

    /// Insert or update an entry in the index.
    pub fn upsert_entry(&self, entry: &IndexEntry) -> AppResult<()> {
        debug!("Upserting index entry");
        let conn = self.conn.lock().map_err(|_| lock_err())?;
        conn.execute(
            r#"
            INSERT OR REPLACE INTO vault_entries
            (path, encrypted_name, is_directory, size, modified_at, etag)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                entry.path,
                entry.encrypted_name,
                entry.is_directory as i32,
                entry.size,
                entry.modified_at,
                entry.etag,
            ],
        )
        .map_err(sqlite_err)?;
        Ok(())
    }

    /// Get an entry by path.
    pub fn get_entry(&self, path: &str) -> AppResult<Option<IndexEntry>> {
        let conn = self.conn.lock().map_err(|_| lock_err())?;
        let mut stmt = conn
            .prepare(
                r#"
            SELECT path, encrypted_name, is_directory, size, modified_at, etag
            FROM vault_entries WHERE path = ?1
            "#,
            )
            .map_err(sqlite_err)?;

        let entry = stmt.query_row([path], |row| {
            Ok(IndexEntry {
                path: row.get(0)?,
                encrypted_name: row.get(1)?,
                is_directory: row.get::<_, i32>(2)? != 0,
                size: row.get::<_, Option<i64>>(3)?,
                modified_at: row.get(4)?,
                etag: row.get(5)?,
            })
        });

        match entry {
            Ok(e) => Ok(Some(e)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(sqlite_err(e)),
        }
    }

    /// List children of a directory.
    pub fn list_children(&self, parent_path: &str) -> AppResult<Vec<IndexEntry>> {
        let prefix = if parent_path == "/" {
            "/".to_string()
        } else {
            format!("{}/", parent_path)
        };

        let conn = self.conn.lock().map_err(|_| lock_err())?;
        let mut stmt = conn
            .prepare(
                r#"
            SELECT path, encrypted_name, is_directory, size, modified_at, etag
            FROM vault_entries
            WHERE path LIKE ?1 AND path != ?2
            "#,
            )
            .map_err(sqlite_err)?;

        let entries = stmt.query_map([format!("{}%", prefix), parent_path.to_string()], |row| {
            let path: String = row.get(0)?;
            let relative = &path[prefix.len()..];
            if !relative.contains('/') {
                Ok(Some(IndexEntry {
                    path,
                    encrypted_name: row.get(1)?,
                    is_directory: row.get::<_, i32>(2)? != 0,
                    size: row.get::<_, Option<i64>>(3)?,
                    modified_at: row.get(4)?,
                    etag: row.get(5)?,
                }))
            } else {
                Ok(None)
            }
        });

        let mut result = Vec::new();
        for entry in entries.map_err(sqlite_err)? {
            if let Some(e) = entry.map_err(sqlite_err)? {
                result.push(e);
            }
        }
        Ok(result)
    }

    /// Delete an entry by path.
    pub fn delete_entry(&self, path: &str) -> AppResult<()> {
        debug!("Deleting index entry");
        let conn = self.conn.lock().map_err(|_| lock_err())?;
        conn.execute("DELETE FROM vault_entries WHERE path = ?1", params![path])
            .map_err(sqlite_err)?;
        Ok(())
    }

    /// Delete all entries under a path (recursively).
    pub fn delete_tree(&self, path: &str) -> AppResult<()> {
        debug!("Deleting index subtree");
        let conn = self.conn.lock().map_err(|_| lock_err())?;
        conn.execute(
            "DELETE FROM vault_entries WHERE path = ?1 OR path LIKE ?2",
            params![path, format!("{}/%", path)],
        )
        .map_err(sqlite_err)?;
        Ok(())
    }

    /// Clear all entries.
    pub fn clear(&self) -> AppResult<()> {
        info!("Clearing local index");
        let conn = self.conn.lock().map_err(|_| lock_err())?;
        conn.execute("DELETE FROM vault_entries", [])
            .map_err(sqlite_err)?;
        Ok(())
    }

    /// Securely wipe all cached data (entries and metadata).
    ///
    /// Called when the vault is locked to ensure no plaintext metadata
    /// persists on disk after the vault is no longer accessible.
    pub fn wipe(&self) -> AppResult<()> {
        info!("Wiping all cached vault metadata from local index");
        let conn = self.conn.lock().map_err(|_| lock_err())?;
        conn.execute("DELETE FROM vault_entries", [])
            .map_err(sqlite_err)?;
        conn.execute("DELETE FROM vault_metadata", [])
            .map_err(sqlite_err)?;
        conn.execute_batch("VACUUM").map_err(sqlite_err)?;
        Ok(())
    }

    /// Get vault metadata value.
    pub fn get_metadata(&self, key: &str) -> AppResult<Option<String>> {
        let conn = self.conn.lock().map_err(|_| lock_err())?;
        let mut stmt = conn
            .prepare("SELECT value FROM vault_metadata WHERE key = ?1")
            .map_err(sqlite_err)?;

        match stmt.query_row([key], |row| row.get(0)) {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(sqlite_err(e)),
        }
    }

    /// Set vault metadata value.
    pub fn set_metadata(&self, key: &str, value: &str) -> AppResult<()> {
        let conn = self.conn.lock().map_err(|_| lock_err())?;
        conn.execute(
            "INSERT OR REPLACE INTO vault_metadata (key, value) VALUES (?1, ?2)",
            params![key, value],
        )
        .map_err(sqlite_err)?;
        Ok(())
    }

    /// Get total entry count.
    pub fn count(&self) -> AppResult<u64> {
        let conn = self.conn.lock().map_err(|_| lock_err())?;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM vault_entries", [], |row| row.get(0))
            .map_err(sqlite_err)?;
        Ok(count as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn open_repairs_unsafe_permissions_on_existing_database() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("index.sqlite");
        std::fs::write(&path, []).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let _index = LocalIndex::open(&path).unwrap();

        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn open_rejects_symlink_database_path() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::TempDir::new().unwrap();
        let victim = dir.path().join("victim");
        std::fs::write(&victim, b"do not modify").unwrap();
        let path = dir.path().join("index.sqlite");
        symlink(&victim, &path).unwrap();

        assert!(LocalIndex::open(&path).is_err());
        assert_eq!(std::fs::read(victim).unwrap(), b"do not modify");
    }

    #[test]
    fn test_index_operations() {
        let index = LocalIndex::in_memory().unwrap();

        let entry = IndexEntry {
            path: "/test.txt".to_string(),
            encrypted_name: "encrypted_name".to_string(),
            is_directory: false,
            size: Some(100),
            modified_at: 1234567890,
            etag: Some("etag123".to_string()),
        };

        index.upsert_entry(&entry).unwrap();
        let retrieved = index.get_entry("/test.txt").unwrap().unwrap();
        assert_eq!(retrieved.path, entry.path);
        assert_eq!(retrieved.size, entry.size);

        index.delete_entry("/test.txt").unwrap();
        assert!(index.get_entry("/test.txt").unwrap().is_none());
    }

    #[test]
    fn test_list_children() {
        let index = LocalIndex::in_memory().unwrap();

        let dir = IndexEntry {
            path: "/mydir".to_string(),
            encrypted_name: "dir_enc".to_string(),
            is_directory: true,
            size: None,
            modified_at: 1234567890,
            etag: None,
        };
        index.upsert_entry(&dir).unwrap();

        let file1 = IndexEntry {
            path: "/mydir/file1.txt".to_string(),
            encrypted_name: "f1_enc".to_string(),
            is_directory: false,
            size: Some(50),
            modified_at: 1234567891,
            etag: None,
        };
        index.upsert_entry(&file1).unwrap();

        let file2 = IndexEntry {
            path: "/mydir/file2.txt".to_string(),
            encrypted_name: "f2_enc".to_string(),
            is_directory: false,
            size: Some(60),
            modified_at: 1234567892,
            etag: None,
        };
        index.upsert_entry(&file2).unwrap();

        let children = index.list_children("/mydir").unwrap();
        assert_eq!(children.len(), 2);
    }

    #[test]
    fn test_metadata() {
        let index = LocalIndex::in_memory().unwrap();

        index.set_metadata("vault_id", "test-vault").unwrap();
        let value = index.get_metadata("vault_id").unwrap().unwrap();
        assert_eq!(value, "test-vault");
    }

    #[test]
    fn test_wipe() {
        let index = LocalIndex::in_memory().unwrap();

        let entry = IndexEntry {
            path: "/secret.txt".to_string(),
            encrypted_name: "enc".to_string(),
            is_directory: false,
            size: Some(42),
            modified_at: 1234567890,
            etag: None,
        };
        index.upsert_entry(&entry).unwrap();
        index.set_metadata("vault_id", "test").unwrap();

        index.wipe().unwrap();

        assert_eq!(index.count().unwrap(), 0);
        assert!(index.get_metadata("vault_id").unwrap().is_none());
    }

    #[test]
    fn test_delete_tree() {
        let index = LocalIndex::in_memory().unwrap();

        for path in &["/dir", "/dir/a.txt", "/dir/b.txt", "/dir/sub/c.txt"] {
            index
                .upsert_entry(&IndexEntry {
                    path: path.to_string(),
                    encrypted_name: "enc".to_string(),
                    is_directory: path.ends_with("dir") || path.ends_with("sub"),
                    size: None,
                    modified_at: 0,
                    etag: None,
                })
                .unwrap();
        }

        index.delete_tree("/dir").unwrap();
        assert_eq!(index.count().unwrap(), 0);
    }
}
