//! Atomic, restrictive writes for plaintext exports and credential files.

#[cfg(unix)]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Controls whether an existing sensitive destination may be replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensitiveFileMode {
    /// Atomically fail if any filesystem entry already exists at the path.
    CreateNew,
    /// Atomically replace an existing regular file; symlinks are rejected.
    Replace,
}

struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn create_private_temp_dir(parent: &Path) -> io::Result<TempDirGuard> {
    for _ in 0..128 {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let count = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".axiomvault-sensitive-{}-{nonce}-{count}",
            std::process::id()
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&path) {
            Ok(()) => return Ok(TempDirGuard(path)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate private temporary directory",
    ))
}

fn open_private_file(path: &Path) -> io::Result<File> {
    #[cfg(not(unix))]
    {
        let _ = path;
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "secure plaintext-file ACL creation is not implemented on this platform",
        ));
    }

    #[cfg(unix)]
    {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        options.open(path)
    }
}

fn sync_parent(parent: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn write_sensitive_with<F>(destination: &Path, mode: SensitiveFileMode, writer: F) -> io::Result<()>
where
    F: FnOnce(&mut File) -> io::Result<()>,
{
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "destination has no file name")
    })?;

    if mode == SensitiveFileMode::Replace {
        match fs::symlink_metadata(destination) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "refusing to replace a symlink destination",
                ));
            }
            Ok(metadata) if !metadata.is_file() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "refusing to replace a non-file destination",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }

    let temp_dir = create_private_temp_dir(parent)?;
    let temp_path = temp_dir.0.join(file_name);
    let mut temp_file = open_private_file(&temp_path)?;
    writer(&mut temp_file)?;
    temp_file.sync_all()?;
    drop(temp_file);

    match mode {
        SensitiveFileMode::CreateNew => {
            fs::hard_link(&temp_path, destination)?;
            fs::remove_file(&temp_path)?;
        }
        SensitiveFileMode::Replace => fs::rename(&temp_path, destination)?,
    }
    sync_parent(parent)?;
    Ok(())
}

/// Write sensitive bytes atomically with mode `0600` on Unix.
///
/// A private `0700` temporary directory is created beside the destination,
/// the file is fully written and fsynced, and only then is it published.
/// `CreateNew` uses an atomic no-clobber link; `Replace` must be explicit and
/// rejects symlink and non-file destinations.
pub fn write_sensitive_file(
    destination: impl AsRef<Path>,
    bytes: &[u8],
    mode: SensitiveFileMode,
) -> io::Result<()> {
    write_sensitive_with(destination.as_ref(), mode, |file| file.write_all(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier, Mutex};

    #[cfg(unix)]
    static UMASK_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    #[cfg(unix)]
    fn permissive_umask_still_creates_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let _lock = UMASK_LOCK.lock().unwrap();
        // SAFETY: the process-global umask is serialized by UMASK_LOCK and restored below.
        let old_umask = unsafe { libc::umask(0) };
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("export");
        let result = write_sensitive_file(&destination, b"plaintext", SensitiveFileMode::CreateNew);
        // SAFETY: restore the exact umask captured while holding UMASK_LOCK.
        unsafe { libc::umask(old_umask) };
        result.unwrap();
        assert_eq!(
            fs::metadata(destination).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    #[cfg(unix)]
    fn attacker_prepared_symlink_is_rejected() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let victim = temp.path().join("victim");
        fs::write(&victim, b"safe").unwrap();
        let destination = temp.path().join("export");
        symlink(&victim, &destination).unwrap();

        assert!(
            write_sensitive_file(&destination, b"secret", SensitiveFileMode::CreateNew).is_err()
        );
        assert!(write_sensitive_file(&destination, b"secret", SensitiveFileMode::Replace).is_err());
        assert_eq!(fs::read(victim).unwrap(), b"safe");
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_creation_has_exactly_one_winner() {
        let temp = tempfile::tempdir().unwrap();
        let destination = Arc::new(temp.path().join("export"));
        let barrier = Arc::new(Barrier::new(3));
        let mut threads = Vec::new();
        for bytes in [b"first".as_slice(), b"second".as_slice()] {
            let destination = destination.clone();
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                write_sensitive_file(destination.as_ref(), bytes, SensitiveFileMode::CreateNew)
            }));
        }
        barrier.wait();
        let successes = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(Result::is_ok)
            .count();
        assert_eq!(successes, 1);
        let content = fs::read(destination.as_ref()).unwrap();
        assert!(content == b"first" || content == b"second");
    }

    #[cfg(unix)]
    #[test]
    fn write_failure_leaves_no_destination_or_temporary_plaintext() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("export");
        let error = write_sensitive_with(&destination, SensitiveFileMode::CreateNew, |file| {
            file.write_all(b"partial secret")?;
            Err(io::Error::other("sabotaged write"))
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "sabotaged write");
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn replacement_requires_explicit_intent_and_remains_restrictive() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("export");
        write_sensitive_file(&destination, b"old", SensitiveFileMode::CreateNew).unwrap();
        assert!(write_sensitive_file(&destination, b"new", SensitiveFileMode::CreateNew).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"old");
        write_sensitive_file(&destination, b"new", SensitiveFileMode::Replace).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"new");
    }

    #[cfg(not(unix))]
    #[test]
    fn unsupported_acl_platform_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("export");
        let error = write_sensitive_file(&destination, b"secret", SensitiveFileMode::CreateNew)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(!destination.exists());
    }
}
