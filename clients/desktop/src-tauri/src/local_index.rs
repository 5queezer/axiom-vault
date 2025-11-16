//! SQLite-based local index for vault metadata caching.
//!
//! Persists vault tree state locally for faster startup and offline access.

use rusqlite::{params, Connection, Result as SqliteResult};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::{debug, info};

/// Represents a cached vault entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub path: String,
    pub encrypted_name: String,
    pub is_directory: bool,
    pub size: Option<u64>,
    pub modified_at: i64,
    pub etag: Option<String>,
}

/// Local index manager using SQLite.
pub struct LocalIndex {
    conn: Connection,
}

impl LocalIndex {
    /// Create or open a local index database.
    ///
    /// # Arguments
    /// - `db_path`: Path to the SQLite database file
    ///
    /// # Errors
    /// - Database creation or migration failure
    pub fn open(db_path: impl AsRef<Path>) -> SqliteResult<Self> {
        let conn = Connection::open(db_path)?;

        // Initialize schema
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
        )?;

        info!("Local index opened successfully");
        Ok(Self { conn })
    }

    /// Create an in-memory index (for testing).
    pub fn in_memory() -> SqliteResult<Self> {
        Self::open(":memory:")
    }

    /// Insert or update an entry in the index.
    pub fn upsert_entry(&self, entry: &IndexEntry) -> SqliteResult<()> {
        debug!("Upserting entry: {}", entry.path);
        self.conn.execute(
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
        )?;
        Ok(())
    }

    /// Get an entry by path.
    pub fn get_entry(&self, path: &str) -> SqliteResult<Option<IndexEntry>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT path, encrypted_name, is_directory, size, modified_at, etag
            FROM vault_entries WHERE path = ?1
            "#,
        )?;

        let entry = stmt.query_row([path], |row| {
            Ok(IndexEntry {
                path: row.get(0)?,
                encrypted_name: row.get(1)?,
                is_directory: row.get::<_, i32>(2)? != 0,
                size: row.get(3)?,
                modified_at: row.get(4)?,
                etag: row.get(5)?,
            })
        });

        match entry {
            Ok(e) => Ok(Some(e)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// List children of a directory.
    pub fn list_children(&self, parent_path: &str) -> SqliteResult<Vec<IndexEntry>> {
        let pattern = if parent_path == "/" {
            "/[^/]+$".to_string()
        } else {
            format!("{}[/][^/]+$", regex::escape(parent_path))
        };

        // Simplified: just get direct children
        let prefix = if parent_path == "/" {
            "/".to_string()
        } else {
            format!("{}/", parent_path)
        };

        let mut stmt = self.conn.prepare(
            r#"
            SELECT path, encrypted_name, is_directory, size, modified_at, etag
            FROM vault_entries
            WHERE path LIKE ?1 AND path != ?2
            "#,
        )?;

        let entries = stmt.query_map([format!("{}%", prefix), parent_path], |row| {
            let path: String = row.get(0)?;
            // Check if it's a direct child (no additional slashes after prefix)
            let relative = &path[prefix.len()..];
            if !relative.contains('/') {
                Ok(Some(IndexEntry {
                    path,
                    encrypted_name: row.get(1)?,
                    is_directory: row.get::<_, i32>(2)? != 0,
                    size: row.get(3)?,
                    modified_at: row.get(4)?,
                    etag: row.get(5)?,
                }))
            } else {
                Ok(None)
            }
        })?;

        let mut result = Vec::new();
        for entry in entries {
            if let Some(e) = entry? {
                result.push(e);
            }
        }
        Ok(result)
    }

    /// Delete an entry by path.
    pub fn delete_entry(&self, path: &str) -> SqliteResult<()> {
        debug!("Deleting entry: {}", path);
        self.conn.execute(
            "DELETE FROM vault_entries WHERE path = ?1",
            params![path],
        )?;
        Ok(())
    }

    /// Delete all entries under a path (recursively).
    pub fn delete_tree(&self, path: &str) -> SqliteResult<()> {
        debug!("Deleting tree: {}", path);
        self.conn.execute(
            "DELETE FROM vault_entries WHERE path = ?1 OR path LIKE ?2",
            params![path, format!("{}/%", path)],
        )?;
        Ok(())
    }

    /// Clear all entries.
    pub fn clear(&self) -> SqliteResult<()> {
        info!("Clearing local index");
        self.conn.execute("DELETE FROM vault_entries", [])?;
        Ok(())
    }

    /// Get vault metadata value.
    pub fn get_metadata(&self, key: &str) -> SqliteResult<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM vault_metadata WHERE key = ?1")?;

        match stmt.query_row([key], |row| row.get(0)) {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Set vault metadata value.
    pub fn set_metadata(&self, key: &str, value: &str) -> SqliteResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO vault_metadata (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    /// Get total entry count.
    pub fn count(&self) -> SqliteResult<u64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM vault_entries", [], |row| row.get(0))?;
        Ok(count as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
