//! Local filesystem storage provider.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::stream;
use std::path::{Path, PathBuf};
use tokio::fs;
use uuid::Uuid;

use crate::provider::{ByteStream, Metadata, StorageProvider};
use axiomvault_common::{Error, Result, VaultPath};

/// File mode for vault files (owner read/write only).
#[cfg(unix)]
const FILE_MODE: u32 = 0o600;
/// Directory mode for vault directories (owner read/write/execute only).
#[cfg(unix)]
const DIR_MODE: u32 = 0o700;

/// Local filesystem storage provider.
///
/// Stores vault data in a local directory structure.
pub struct LocalProvider {
    root: PathBuf,
}

impl LocalProvider {
    /// Create a new local provider with the given root directory.
    ///
    /// # Preconditions
    /// - Root path must be a valid directory path
    ///
    /// # Postconditions
    /// - Provider is ready to use
    /// - Root directory is created if it doesn't exist
    /// - On Unix, a newly created root directory is restricted to mode `0o700`
    ///
    /// # Errors
    /// - Invalid path
    /// - Permission denied
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();

        // Create root if it doesn't exist (sync for constructor).
        // On Unix, restrict mode to 0o700 so other local users cannot read
        // wrapped keys, KDF parameters, or ciphertext sitting at rest.
        if !root.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(DIR_MODE)
                    .create(&root)?;
            }
            #[cfg(not(unix))]
            {
                std::fs::create_dir_all(&root)?;
            }
        }

        // Defense in depth: if the root already existed (or an adversary
        // pre-created it in the gap between `exists()` and `create`), we
        // never applied DIR_MODE. With `recursive(true)`, `create` succeeds
        // silently against a pre-existing weak-mode directory. Read the
        // actual mode now and repair it if it differs. This does NOT close
        // the creation race, but it ensures any adversary's pre-emptive
        // weak-mode directory gets re-permissioned before we trust it for
        // wrapped keys / KDF parameters / ciphertext.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&root)?;
            let current_mode = meta.permissions().mode() & 0o777;
            if current_mode != DIR_MODE {
                std::fs::set_permissions(&root, std::fs::Permissions::from_mode(DIR_MODE))?;
            }
        }

        Ok(Self { root })
    }

    /// Convert a VaultPath to a filesystem path.
    fn to_fs_path(&self, path: &VaultPath) -> PathBuf {
        let mut fs_path = self.root.clone();
        for component in path.components() {
            fs_path.push(component);
        }
        fs_path
    }

    /// Create metadata from filesystem metadata.
    fn create_metadata(&self, path: &VaultPath, fs_meta: std::fs::Metadata) -> Metadata {
        let modified: DateTime<Utc> = fs_meta
            .modified()
            .map(|t| t.into())
            .unwrap_or_else(|_| Utc::now());

        Metadata {
            id: Uuid::new_v4().to_string(),
            name: path.name().unwrap_or("/").to_string(),
            size: if fs_meta.is_file() {
                Some(fs_meta.len())
            } else {
                None
            },
            is_directory: fs_meta.is_dir(),
            modified,
            etag: Some(format!("{}-{}", modified.timestamp(), fs_meta.len())),
            provider_data: None,
        }
    }
}

#[async_trait]
impl StorageProvider for LocalProvider {
    fn name(&self) -> &str {
        "local"
    }

    async fn upload(&self, path: &VaultPath, data: Vec<u8>) -> Result<Metadata> {
        let fs_path = self.to_fs_path(path);

        // Check parent exists
        if let Some(parent) = fs_path.parent() {
            if !parent.exists() {
                return Err(Error::NotFound("Parent directory not found".to_string()));
            }
        }

        // Write atomically: write to a temp file in the same directory, then rename.
        // This prevents partial/corrupt files if the process is interrupted mid-write.
        let parent_dir = fs_path
            .parent()
            .ok_or_else(|| Error::InvalidInput("Cannot write to root path".to_string()))?;
        let tmp_path = parent_dir.join(format!(
            ".{}.tmp.{}",
            fs_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file"),
            uuid::Uuid::new_v4()
        ));

        // Create the temp file with restrictive permissions on Unix so
        // other local users cannot read ciphertext or vault config that
        // contains wrapped keys / KDF parameters. On non-Unix targets we
        // fall back to the default permissions (the file will inherit
        // platform defaults).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            use tokio::io::AsyncWriteExt;

            // tokio::fs::OpenOptions exposes `mode()` directly on Unix
            // (cfg-gated to the unix targets), so no extension trait
            // import is required.
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(FILE_MODE)
                .open(&tmp_path)
                .await?;
            tokio::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(FILE_MODE))
                .await?;
            if let Err(e) = file.write_all(&data).await {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(e.into());
            }
            if let Err(e) = file.sync_all().await {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(e.into());
            }
        }
        #[cfg(not(unix))]
        {
            // Match the Unix branch's durability semantics: explicit
            // create -> write_all -> sync_all. `fs::write` skips the
            // fsync, so on power loss the rename target could resolve to
            // a zero-length file. Keep both platforms aligned.
            use tokio::io::AsyncWriteExt;
            let mut file = fs::File::create(&tmp_path).await?;
            if let Err(e) = file.write_all(&data).await {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(e.into());
            }
            if let Err(e) = file.sync_all().await {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(e.into());
            }
        }

        if let Err(e) = fs::rename(&tmp_path, &fs_path).await {
            // Best-effort cleanup; ignore secondary error
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e.into());
        }

        let fs_meta = fs::metadata(&fs_path).await?;
        Ok(self.create_metadata(path, fs_meta))
    }

    async fn upload_stream(&self, path: &VaultPath, mut stream: ByteStream) -> Result<Metadata> {
        use futures::StreamExt;
        let mut data = Vec::new();

        while let Some(chunk) = stream.next().await {
            data.extend_from_slice(&chunk?);
        }

        self.upload(path, data).await
    }

    async fn download(&self, path: &VaultPath) -> Result<Vec<u8>> {
        let fs_path = self.to_fs_path(path);

        if !fs_path.exists() {
            return Err(Error::NotFound(format!("File not found: {}", path)));
        }

        if fs_path.is_dir() {
            return Err(Error::InvalidInput("Cannot download directory".to_string()));
        }

        Ok(fs::read(&fs_path).await?)
    }

    async fn download_stream(&self, path: &VaultPath) -> Result<ByteStream> {
        let data = self.download(path).await?;
        let stream = stream::once(async move { Ok(data) });
        Ok(Box::pin(stream))
    }

    async fn exists(&self, path: &VaultPath) -> Result<bool> {
        let fs_path = self.to_fs_path(path);
        Ok(fs_path.exists())
    }

    async fn delete(&self, path: &VaultPath) -> Result<()> {
        let fs_path = self.to_fs_path(path);

        if !fs_path.exists() {
            return Err(Error::NotFound(format!("File not found: {}", path)));
        }

        if fs_path.is_dir() {
            return Err(Error::InvalidInput(
                "Use delete_dir for directories".to_string(),
            ));
        }

        fs::remove_file(&fs_path).await?;
        Ok(())
    }

    async fn list(&self, path: &VaultPath) -> Result<Vec<Metadata>> {
        let fs_path = self.to_fs_path(path);

        if !fs_path.exists() {
            return Err(Error::NotFound(format!("Directory not found: {}", path)));
        }

        if !fs_path.is_dir() {
            return Err(Error::InvalidInput("Not a directory".to_string()));
        }

        let mut results = Vec::new();
        let mut entries = fs::read_dir(&fs_path).await?;

        while let Some(entry) = entries.next_entry().await? {
            let entry_path = entry.path();
            let name = entry_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            let child_vault_path = path.join(&name)?;
            let fs_meta = entry.metadata().await?;
            results.push(self.create_metadata(&child_vault_path, fs_meta));
        }

        Ok(results)
    }

    async fn metadata(&self, path: &VaultPath) -> Result<Metadata> {
        let fs_path = self.to_fs_path(path);

        if !fs_path.exists() {
            return Err(Error::NotFound(format!("Path not found: {}", path)));
        }

        let fs_meta = fs::metadata(&fs_path).await?;
        Ok(self.create_metadata(path, fs_meta))
    }

    async fn create_dir(&self, path: &VaultPath) -> Result<Metadata> {
        let fs_path = self.to_fs_path(path);

        if fs_path.exists() {
            return Err(Error::AlreadyExists(format!(
                "Path already exists: {}",
                path
            )));
        }

        // On Unix, set the mode atomically at directory creation via
        // `DirBuilder::mode` instead of `create_dir` followed by
        // `set_permissions`. The latter leaves a brief window where the
        // directory exists with umask-affected permissions and another
        // local user could enter or list it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            let fs_path_blocking = fs_path.clone();
            tokio::task::spawn_blocking(move || {
                std::fs::DirBuilder::new()
                    .mode(DIR_MODE)
                    .create(&fs_path_blocking)?;
                std::fs::set_permissions(
                    &fs_path_blocking,
                    std::fs::Permissions::from_mode(DIR_MODE),
                )
            })
            .await
            .map_err(|e| Error::Storage(format!("spawn_blocking join error: {}", e)))??;
        }
        #[cfg(not(unix))]
        {
            fs::create_dir(&fs_path).await?;
        }

        let fs_meta = fs::metadata(&fs_path).await?;
        Ok(self.create_metadata(path, fs_meta))
    }

    async fn delete_dir(&self, path: &VaultPath) -> Result<()> {
        let fs_path = self.to_fs_path(path);

        if !fs_path.exists() {
            return Err(Error::NotFound(format!("Directory not found: {}", path)));
        }

        if !fs_path.is_dir() {
            return Err(Error::InvalidInput("Not a directory".to_string()));
        }

        // Check if empty
        let mut entries = fs::read_dir(&fs_path).await?;
        if entries.next_entry().await?.is_some() {
            return Err(Error::InvalidInput("Directory not empty".to_string()));
        }

        fs::remove_dir(&fs_path).await?;
        Ok(())
    }

    async fn rename(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        let from_path = self.to_fs_path(from);
        let to_path = self.to_fs_path(to);

        if !from_path.exists() {
            return Err(Error::NotFound(format!("Source not found: {}", from)));
        }

        if to_path.exists() {
            return Err(Error::AlreadyExists(format!(
                "Destination already exists: {}",
                to
            )));
        }

        fs::rename(&from_path, &to_path).await?;

        let fs_meta = fs::metadata(&to_path).await?;
        Ok(self.create_metadata(to, fs_meta))
    }

    async fn copy(&self, from: &VaultPath, to: &VaultPath) -> Result<Metadata> {
        let from_path = self.to_fs_path(from);
        let to_path = self.to_fs_path(to);

        if !from_path.exists() {
            return Err(Error::NotFound(format!("Source not found: {}", from)));
        }

        if to_path.exists() {
            return Err(Error::AlreadyExists(format!(
                "Destination already exists: {}",
                to
            )));
        }

        if from_path.is_dir() {
            // Create the destination directory with restrictive mode set
            // atomically at creation (Unix). Avoids the TOCTOU window where
            // `create_dir` followed by `set_permissions` would briefly
            // expose the new directory at the umask-affected mode.
            #[cfg(unix)]
            {
                use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
                let to_blocking = to_path.clone();
                tokio::task::spawn_blocking(move || {
                    std::fs::DirBuilder::new()
                        .mode(DIR_MODE)
                        .create(&to_blocking)?;
                    std::fs::set_permissions(
                        &to_blocking,
                        std::fs::Permissions::from_mode(DIR_MODE),
                    )
                })
                .await
                .map_err(|e| Error::Storage(format!("spawn_blocking join error: {}", e)))??;
            }
            #[cfg(not(unix))]
            {
                fs::create_dir(&to_path).await?;
            }
        } else {
            // Stream the source into a freshly-created destination instead of
            // loading the whole vault object into memory. The Unix path keeps
            // restrictive creation mode and then repairs exact bits in case a
            // restrictive umask removed owner permissions.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut source = fs::File::open(&from_path).await?;
                let mut destination = tokio::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(FILE_MODE)
                    .open(&to_path)
                    .await?;
                tokio::fs::set_permissions(&to_path, std::fs::Permissions::from_mode(FILE_MODE))
                    .await?;
                if let Err(e) = tokio::io::copy(&mut source, &mut destination).await {
                    let _ = std::fs::remove_file(&to_path);
                    return Err(e.into());
                }
                if let Err(e) = destination.sync_all().await {
                    let _ = std::fs::remove_file(&to_path);
                    return Err(e.into());
                }
            }
            #[cfg(not(unix))]
            {
                let mut source = fs::File::open(&from_path).await?;
                let mut destination = fs::File::create_new(&to_path).await?;
                if let Err(e) = tokio::io::copy(&mut source, &mut destination).await {
                    let _ = std::fs::remove_file(&to_path);
                    return Err(e.into());
                }
                if let Err(e) = destination.sync_all().await {
                    let _ = std::fs::remove_file(&to_path);
                    return Err(e.into());
                }
            }
        }

        let fs_meta = fs::metadata(&to_path).await?;
        Ok(self.create_metadata(to, fs_meta))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_local_upload_download() {
        let temp = TempDir::new().unwrap();
        let provider = LocalProvider::new(temp.path()).unwrap();
        let path = VaultPath::parse("/test.txt").unwrap();
        let data = b"Hello, Local!".to_vec();

        provider.upload(&path, data.clone()).await.unwrap();
        let downloaded = provider.download(&path).await.unwrap();

        assert_eq!(downloaded, data);
    }

    #[tokio::test]
    async fn test_local_create_dir() {
        let temp = TempDir::new().unwrap();
        let provider = LocalProvider::new(temp.path()).unwrap();
        let path = VaultPath::parse("/mydir").unwrap();

        let metadata = provider.create_dir(&path).await.unwrap();
        assert!(metadata.is_directory);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_local_upload_sets_restrictive_file_mode() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let provider = LocalProvider::new(temp.path()).unwrap();
        let path = VaultPath::parse("/secret.bin").unwrap();
        provider
            .upload(&path, b"ciphertext".to_vec())
            .await
            .unwrap();

        let fs_path = temp.path().join("secret.bin");
        let mode = std::fs::metadata(&fs_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "uploaded file must be owner-only readable");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_local_create_dir_sets_restrictive_dir_mode() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let provider = LocalProvider::new(temp.path()).unwrap();
        let path = VaultPath::parse("/private").unwrap();
        provider.create_dir(&path).await.unwrap();

        let fs_path = temp.path().join("private");
        let mode = std::fs::metadata(&fs_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "created directory must be owner-only");
    }

    #[cfg(unix)]
    #[test]
    fn test_local_new_creates_root_with_restrictive_dir_mode() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let root = temp.path().join("vault-root");
        assert!(!root.exists());
        LocalProvider::new(&root).unwrap();

        let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "newly created vault root must be owner-only");
    }

    /// Defense-in-depth: when the root directory already exists (e.g. an
    /// adversary pre-created it with a permissive mode in the gap between
    /// our `exists()` check and our `DirBuilder::create`, or it was just
    /// left over from a prior run with the wrong mode), `LocalProvider::new`
    /// must repair the mode to `DIR_MODE` (0o700) before returning.
    #[cfg(unix)]
    #[test]
    fn test_local_new_repairs_pre_existing_root_mode() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let root = temp.path().join("vault-root");

        // Pre-create the root with a world-readable mode, simulating either
        // a leftover directory or an adversary winning the TOCTOU race.
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        let pre_mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
        assert_eq!(pre_mode, 0o755, "test setup must produce 0o755");

        LocalProvider::new(&root).unwrap();

        let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o700,
            "pre-existing root with weak mode must be repaired to 0o700"
        );
    }

    /// Audit hardening: `copy` of a regular file must produce a destination
    /// with mode `0o600` from the moment of creation. The pre-fix
    /// implementation used `fs::copy` then `set_permissions`, leaving a
    /// TOCTOU window where another local user could open the destination at
    /// the umask-affected mode.
    #[cfg(unix)]
    #[tokio::test]
    async fn test_local_copy_file_sets_restrictive_file_mode() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let provider = LocalProvider::new(temp.path()).unwrap();
        let src = VaultPath::parse("/src.bin").unwrap();
        let dst = VaultPath::parse("/dst.bin").unwrap();
        provider.upload(&src, b"ciphertext".to_vec()).await.unwrap();
        provider.copy(&src, &dst).await.unwrap();

        let fs_path = temp.path().join("dst.bin");
        let mode = std::fs::metadata(&fs_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "copied file must be owner-only readable");

        // Content must still match.
        let copied = provider.download(&dst).await.unwrap();
        assert_eq!(copied, b"ciphertext");
    }

    /// Audit hardening: `copy` of a directory must produce a destination
    /// with mode `0o700` set atomically at creation (no TOCTOU window).
    #[cfg(unix)]
    #[tokio::test]
    async fn test_local_copy_dir_sets_restrictive_dir_mode() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let provider = LocalProvider::new(temp.path()).unwrap();
        let src = VaultPath::parse("/srcdir").unwrap();
        let dst = VaultPath::parse("/dstdir").unwrap();
        provider.create_dir(&src).await.unwrap();
        provider.copy(&src, &dst).await.unwrap();

        let fs_path = temp.path().join("dstdir");
        let mode = std::fs::metadata(&fs_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "copied directory must be owner-only");
    }

    #[tokio::test]
    async fn test_local_list() {
        let temp = TempDir::new().unwrap();
        let provider = LocalProvider::new(temp.path()).unwrap();

        provider
            .create_dir(&VaultPath::parse("/dir").unwrap())
            .await
            .unwrap();
        provider
            .upload(&VaultPath::parse("/dir/file1.txt").unwrap(), vec![1])
            .await
            .unwrap();
        provider
            .upload(&VaultPath::parse("/dir/file2.txt").unwrap(), vec![2])
            .await
            .unwrap();

        let contents = provider
            .list(&VaultPath::parse("/dir").unwrap())
            .await
            .unwrap();
        assert_eq!(contents.len(), 2);
    }
}
