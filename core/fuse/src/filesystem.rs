//! FUSE filesystem implementation for AxiomVault.
//!
//! Implements the fuser::Filesystem trait to expose an encrypted vault
//! as a standard filesystem.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow,
};
use tokio::runtime::Handle;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

use axiomvault_common::{Result, VaultPath};
use axiomvault_vault::{VaultOperations, VaultSession};

/// Inode number mapping to vault paths.
struct InodeMap {
    path_to_inode: HashMap<String, u64>,
    inode_to_path: HashMap<u64, String>,
    next_inode: u64,
}

impl InodeMap {
    fn new() -> Self {
        let mut map = Self {
            path_to_inode: HashMap::new(),
            inode_to_path: HashMap::new(),
            next_inode: 2, // 1 is reserved for root
        };
        // Root inode
        map.path_to_inode.insert("/".to_string(), 1);
        map.inode_to_path.insert(1, "/".to_string());
        map
    }

    fn get_or_create_inode(&mut self, path: &str) -> u64 {
        if let Some(&ino) = self.path_to_inode.get(path) {
            ino
        } else {
            let ino = self.next_inode;
            self.next_inode += 1;
            self.path_to_inode.insert(path.to_string(), ino);
            self.inode_to_path.insert(ino, path.to_string());
            ino
        }
    }

    fn get_path(&self, inode: u64) -> Option<&str> {
        self.inode_to_path.get(&inode).map(|s| s.as_str())
    }

    fn remove_inode(&mut self, path: &str) {
        if let Some(ino) = self.path_to_inode.remove(path) {
            self.inode_to_path.remove(&ino);
        }
    }
}

/// File handle tracking for open files.
struct OpenFile {
    path: String,
    buffer: Vec<u8>,
    dirty: bool,
}

/// FUSE filesystem implementation for an encrypted vault.
///
/// This translates FUSE operations into vault operations, handling
/// encryption and decryption transparently.
pub struct VaultFilesystem {
    session: Arc<VaultSession>,
    runtime: Handle,
    inodes: Arc<RwLock<InodeMap>>,
    open_files: Arc<RwLock<HashMap<u64, OpenFile>>>,
    next_fh: Arc<RwLock<u64>>,
    ttl: Duration,
}

impl VaultFilesystem {
    /// Create a new FUSE filesystem for a vault session.
    ///
    /// # Preconditions
    /// - Session must be active
    /// - Runtime handle must be valid
    ///
    /// # Arguments
    /// - `session`: Active vault session
    /// - `runtime`: Tokio runtime handle for async operations
    pub fn new(session: Arc<VaultSession>, runtime: Handle) -> Self {
        Self {
            session,
            runtime,
            inodes: Arc::new(RwLock::new(InodeMap::new())),
            open_files: Arc::new(RwLock::new(HashMap::new())),
            next_fh: Arc::new(RwLock::new(1)),
            ttl: Duration::from_secs(1),
        }
    }

    /// Convert vault metadata to FUSE file attributes.
    fn create_attr(&self, ino: u64, is_dir: bool, size: u64) -> FileAttr {
        let now = SystemTime::now();
        FileAttr {
            ino,
            size,
            blocks: (size + 511) / 512,
            atime: now,
            mtime: now,
            ctime: now,
            crtime: now,
            kind: if is_dir {
                FileType::Directory
            } else {
                FileType::RegularFile
            },
            perm: if is_dir { 0o755 } else { 0o644 },
            nlink: if is_dir { 2 } else { 1 },
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    /// Get vault operations handler.
    fn ops(&self) -> Result<VaultOperations<'_>> {
        VaultOperations::new(&self.session)
    }
}

impl Filesystem for VaultFilesystem {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        debug!("lookup: parent={}, name={}", parent, name_str);

        let session = self.session.clone();
        let inodes = self.inodes.clone();
        let ttl = self.ttl;

        self.runtime.block_on(async move {
            let parent_path = {
                let map = inodes.read().await;
                match map.get_path(parent) {
                    Some(p) => p.to_string(),
                    None => {
                        reply.error(libc::ENOENT);
                        return;
                    }
                }
            };

            let child_path = if parent_path == "/" {
                format!("/{}", name_str)
            } else {
                format!("{}/{}", parent_path, name_str)
            };

            let ops = match VaultOperations::new(&session) {
                Ok(o) => o,
                Err(_) => {
                    reply.error(libc::EIO);
                    return;
                }
            };

            let path = match VaultPath::parse(&child_path) {
                Ok(p) => p,
                Err(_) => {
                    reply.error(libc::ENOENT);
                    return;
                }
            };

            if !ops.exists(&path).await {
                reply.error(libc::ENOENT);
                return;
            }

            match ops.metadata(&path).await {
                Ok((_, is_dir, size)) => {
                    let mut map = inodes.write().await;
                    let ino = map.get_or_create_inode(&child_path);
                    let attr = FileAttr {
                        ino,
                        size: size.unwrap_or(0),
                        blocks: (size.unwrap_or(0) + 511) / 512,
                        atime: SystemTime::now(),
                        mtime: SystemTime::now(),
                        ctime: SystemTime::now(),
                        crtime: SystemTime::now(),
                        kind: if is_dir {
                            FileType::Directory
                        } else {
                            FileType::RegularFile
                        },
                        perm: if is_dir { 0o755 } else { 0o644 },
                        nlink: if is_dir { 2 } else { 1 },
                        uid: unsafe { libc::getuid() },
                        gid: unsafe { libc::getgid() },
                        rdev: 0,
                        blksize: 4096,
                        flags: 0,
                    };
                    reply.entry(&ttl, &attr, 0);
                }
                Err(e) => {
                    error!("Failed to get metadata: {}", e);
                    reply.error(libc::EIO);
                }
            }
        });
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        debug!("getattr: ino={}", ino);

        let session = self.session.clone();
        let inodes = self.inodes.clone();
        let ttl = self.ttl;

        self.runtime.block_on(async move {
            let path_str = {
                let map = inodes.read().await;
                match map.get_path(ino) {
                    Some(p) => p.to_string(),
                    None => {
                        reply.error(libc::ENOENT);
                        return;
                    }
                }
            };

            if path_str == "/" {
                // Root directory
                let attr = FileAttr {
                    ino: 1,
                    size: 0,
                    blocks: 0,
                    atime: SystemTime::now(),
                    mtime: SystemTime::now(),
                    ctime: SystemTime::now(),
                    crtime: SystemTime::now(),
                    kind: FileType::Directory,
                    perm: 0o755,
                    nlink: 2,
                    uid: unsafe { libc::getuid() },
                    gid: unsafe { libc::getgid() },
                    rdev: 0,
                    blksize: 4096,
                    flags: 0,
                };
                reply.attr(&ttl, &attr);
                return;
            }

            let ops = match VaultOperations::new(&session) {
                Ok(o) => o,
                Err(_) => {
                    reply.error(libc::EIO);
                    return;
                }
            };

            let path = match VaultPath::parse(&path_str) {
                Ok(p) => p,
                Err(_) => {
                    reply.error(libc::ENOENT);
                    return;
                }
            };

            match ops.metadata(&path).await {
                Ok((_, is_dir, size)) => {
                    let attr = FileAttr {
                        ino,
                        size: size.unwrap_or(0),
                        blocks: (size.unwrap_or(0) + 511) / 512,
                        atime: SystemTime::now(),
                        mtime: SystemTime::now(),
                        ctime: SystemTime::now(),
                        crtime: SystemTime::now(),
                        kind: if is_dir {
                            FileType::Directory
                        } else {
                            FileType::RegularFile
                        },
                        perm: if is_dir { 0o755 } else { 0o644 },
                        nlink: if is_dir { 2 } else { 1 },
                        uid: unsafe { libc::getuid() },
                        gid: unsafe { libc::getgid() },
                        rdev: 0,
                        blksize: 4096,
                        flags: 0,
                    };
                    reply.attr(&ttl, &attr);
                }
                Err(e) => {
                    error!("Failed to get metadata: {}", e);
                    reply.error(libc::EIO);
                }
            }
        });
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        debug!("readdir: ino={}, offset={}", ino, offset);

        let session = self.session.clone();
        let inodes = self.inodes.clone();

        self.runtime.block_on(async move {
            let path_str = {
                let map = inodes.read().await;
                match map.get_path(ino) {
                    Some(p) => p.to_string(),
                    None => {
                        reply.error(libc::ENOENT);
                        return;
                    }
                }
            };

            let ops = match VaultOperations::new(&session) {
                Ok(o) => o,
                Err(_) => {
                    reply.error(libc::EIO);
                    return;
                }
            };

            let path = match VaultPath::parse(&path_str) {
                Ok(p) => p,
                Err(_) => {
                    reply.error(libc::ENOENT);
                    return;
                }
            };

            let entries = match ops.list_directory(&path).await {
                Ok(e) => e,
                Err(e) => {
                    error!("Failed to list directory: {}", e);
                    reply.error(libc::EIO);
                    return;
                }
            };

            let mut i = offset as usize;

            // . and .. entries
            if i == 0 {
                if reply.add(ino, 1, FileType::Directory, ".") {
                    reply.ok();
                    return;
                }
                i += 1;
            }
            if i == 1 {
                let parent_ino = if ino == 1 { 1 } else { 1 }; // Simplified parent lookup
                if reply.add(parent_ino, 2, FileType::Directory, "..") {
                    reply.ok();
                    return;
                }
                i += 1;
            }

            // Add regular entries
            for (idx, (name, is_dir, _)) in entries.iter().enumerate().skip(i.saturating_sub(2)) {
                let child_path = if path_str == "/" {
                    format!("/{}", name)
                } else {
                    format!("{}/{}", path_str, name)
                };

                let child_ino = {
                    let mut map = inodes.write().await;
                    map.get_or_create_inode(&child_path)
                };

                let file_type = if *is_dir {
                    FileType::Directory
                } else {
                    FileType::RegularFile
                };

                if reply.add(child_ino, (idx + 3) as i64, file_type, name) {
                    break;
                }
            }

            reply.ok();
        });
    }

    fn open(&mut self, _req: &Request, ino: u64, flags: i32, reply: ReplyOpen) {
        debug!("open: ino={}, flags={}", ino, flags);

        let session = self.session.clone();
        let inodes = self.inodes.clone();
        let open_files = self.open_files.clone();
        let next_fh = self.next_fh.clone();

        self.runtime.block_on(async move {
            let path_str = {
                let map = inodes.read().await;
                match map.get_path(ino) {
                    Some(p) => p.to_string(),
                    None => {
                        reply.error(libc::ENOENT);
                        return;
                    }
                }
            };

            let ops = match VaultOperations::new(&session) {
                Ok(o) => o,
                Err(_) => {
                    reply.error(libc::EIO);
                    return;
                }
            };

            let path = match VaultPath::parse(&path_str) {
                Ok(p) => p,
                Err(_) => {
                    reply.error(libc::ENOENT);
                    return;
                }
            };

            // Read file content into buffer
            let buffer = match ops.read_file(&path).await {
                Ok(data) => data,
                Err(e) => {
                    error!("Failed to read file: {}", e);
                    reply.error(libc::EIO);
                    return;
                }
            };

            let fh = {
                let mut fh_guard = next_fh.write().await;
                let handle = *fh_guard;
                *fh_guard += 1;
                handle
            };

            {
                let mut files = open_files.write().await;
                files.insert(
                    fh,
                    OpenFile {
                        path: path_str,
                        buffer,
                        dirty: false,
                    },
                );
            }

            reply.opened(fh, 0);
        });
    }

    fn read(
        &mut self,
        _req: &Request,
        _ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        debug!("read: fh={}, offset={}, size={}", fh, offset, size);

        let open_files = self.open_files.clone();

        self.runtime.block_on(async move {
            let files = open_files.read().await;
            match files.get(&fh) {
                Some(file) => {
                    let offset = offset as usize;
                    let end = (offset + size as usize).min(file.buffer.len());
                    if offset >= file.buffer.len() {
                        reply.data(&[]);
                    } else {
                        reply.data(&file.buffer[offset..end]);
                    }
                }
                None => {
                    reply.error(libc::EBADF);
                }
            }
        });
    }

    fn write(
        &mut self,
        _req: &Request,
        _ino: u64,
        fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyWrite,
    ) {
        debug!("write: fh={}, offset={}, size={}", fh, offset, data.len());

        let open_files = self.open_files.clone();

        self.runtime.block_on(async move {
            let mut files = open_files.write().await;
            match files.get_mut(&fh) {
                Some(file) => {
                    let offset = offset as usize;
                    let end = offset + data.len();

                    // Extend buffer if necessary
                    if end > file.buffer.len() {
                        file.buffer.resize(end, 0);
                    }

                    file.buffer[offset..end].copy_from_slice(data);
                    file.dirty = true;

                    reply.written(data.len() as u32);
                }
                None => {
                    reply.error(libc::EBADF);
                }
            }
        });
    }

    fn release(
        &mut self,
        _req: &Request,
        _ino: u64,
        fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        debug!("release: fh={}", fh);

        let session = self.session.clone();
        let open_files = self.open_files.clone();

        self.runtime.block_on(async move {
            let file = {
                let mut files = open_files.write().await;
                files.remove(&fh)
            };

            if let Some(file) = file {
                if file.dirty {
                    let ops = match VaultOperations::new(&session) {
                        Ok(o) => o,
                        Err(e) => {
                            error!("Failed to get operations: {}", e);
                            reply.error(libc::EIO);
                            return;
                        }
                    };

                    let path = match VaultPath::parse(&file.path) {
                        Ok(p) => p,
                        Err(e) => {
                            error!("Invalid path: {}", e);
                            reply.error(libc::EIO);
                            return;
                        }
                    };

                    if let Err(e) = ops.update_file(&path, &file.buffer).await {
                        error!("Failed to write file: {}", e);
                        reply.error(libc::EIO);
                        return;
                    }

                    info!("File saved: {}", file.path);
                }
            }

            reply.ok();
        });
    }

    fn create(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        debug!("create: parent={}, name={}", parent, name_str);

        let session = self.session.clone();
        let inodes = self.inodes.clone();
        let open_files = self.open_files.clone();
        let next_fh = self.next_fh.clone();
        let ttl = self.ttl;

        self.runtime.block_on(async move {
            let parent_path = {
                let map = inodes.read().await;
                match map.get_path(parent) {
                    Some(p) => p.to_string(),
                    None => {
                        reply.error(libc::ENOENT);
                        return;
                    }
                }
            };

            let child_path = if parent_path == "/" {
                format!("/{}", name_str)
            } else {
                format!("{}/{}", parent_path, name_str)
            };

            let ops = match VaultOperations::new(&session) {
                Ok(o) => o,
                Err(_) => {
                    reply.error(libc::EIO);
                    return;
                }
            };

            let path = match VaultPath::parse(&child_path) {
                Ok(p) => p,
                Err(_) => {
                    reply.error(libc::EINVAL);
                    return;
                }
            };

            // Create empty file
            if let Err(e) = ops.create_file(&path, &[]).await {
                error!("Failed to create file: {}", e);
                reply.error(libc::EIO);
                return;
            }

            let ino = {
                let mut map = inodes.write().await;
                map.get_or_create_inode(&child_path)
            };

            let fh = {
                let mut fh_guard = next_fh.write().await;
                let handle = *fh_guard;
                *fh_guard += 1;
                handle
            };

            {
                let mut files = open_files.write().await;
                files.insert(
                    fh,
                    OpenFile {
                        path: child_path,
                        buffer: vec![],
                        dirty: false,
                    },
                );
            }

            let attr = FileAttr {
                ino,
                size: 0,
                blocks: 0,
                atime: SystemTime::now(),
                mtime: SystemTime::now(),
                ctime: SystemTime::now(),
                crtime: SystemTime::now(),
                kind: FileType::RegularFile,
                perm: 0o644,
                nlink: 1,
                uid: unsafe { libc::getuid() },
                gid: unsafe { libc::getgid() },
                rdev: 0,
                blksize: 4096,
                flags: 0,
            };

            reply.created(&ttl, &attr, 0, fh, 0);
        });
    }

    fn mkdir(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        debug!("mkdir: parent={}, name={}", parent, name_str);

        let session = self.session.clone();
        let inodes = self.inodes.clone();
        let ttl = self.ttl;

        self.runtime.block_on(async move {
            let parent_path = {
                let map = inodes.read().await;
                match map.get_path(parent) {
                    Some(p) => p.to_string(),
                    None => {
                        reply.error(libc::ENOENT);
                        return;
                    }
                }
            };

            let child_path = if parent_path == "/" {
                format!("/{}", name_str)
            } else {
                format!("{}/{}", parent_path, name_str)
            };

            let ops = match VaultOperations::new(&session) {
                Ok(o) => o,
                Err(_) => {
                    reply.error(libc::EIO);
                    return;
                }
            };

            let path = match VaultPath::parse(&child_path) {
                Ok(p) => p,
                Err(_) => {
                    reply.error(libc::EINVAL);
                    return;
                }
            };

            if let Err(e) = ops.create_directory(&path).await {
                error!("Failed to create directory: {}", e);
                reply.error(libc::EIO);
                return;
            }

            let ino = {
                let mut map = inodes.write().await;
                map.get_or_create_inode(&child_path)
            };

            let attr = FileAttr {
                ino,
                size: 0,
                blocks: 0,
                atime: SystemTime::now(),
                mtime: SystemTime::now(),
                ctime: SystemTime::now(),
                crtime: SystemTime::now(),
                kind: FileType::Directory,
                perm: 0o755,
                nlink: 2,
                uid: unsafe { libc::getuid() },
                gid: unsafe { libc::getgid() },
                rdev: 0,
                blksize: 4096,
                flags: 0,
            };

            reply.entry(&ttl, &attr, 0);
        });
    }

    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        debug!("unlink: parent={}, name={}", parent, name_str);

        let session = self.session.clone();
        let inodes = self.inodes.clone();

        self.runtime.block_on(async move {
            let parent_path = {
                let map = inodes.read().await;
                match map.get_path(parent) {
                    Some(p) => p.to_string(),
                    None => {
                        reply.error(libc::ENOENT);
                        return;
                    }
                }
            };

            let child_path = if parent_path == "/" {
                format!("/{}", name_str)
            } else {
                format!("{}/{}", parent_path, name_str)
            };

            let ops = match VaultOperations::new(&session) {
                Ok(o) => o,
                Err(_) => {
                    reply.error(libc::EIO);
                    return;
                }
            };

            let path = match VaultPath::parse(&child_path) {
                Ok(p) => p,
                Err(_) => {
                    reply.error(libc::EINVAL);
                    return;
                }
            };

            if let Err(e) = ops.delete_file(&path).await {
                error!("Failed to delete file: {}", e);
                reply.error(libc::EIO);
                return;
            }

            {
                let mut map = inodes.write().await;
                map.remove_inode(&child_path);
            }

            reply.ok();
        });
    }

    fn rmdir(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };

        debug!("rmdir: parent={}, name={}", parent, name_str);

        let session = self.session.clone();
        let inodes = self.inodes.clone();

        self.runtime.block_on(async move {
            let parent_path = {
                let map = inodes.read().await;
                match map.get_path(parent) {
                    Some(p) => p.to_string(),
                    None => {
                        reply.error(libc::ENOENT);
                        return;
                    }
                }
            };

            let child_path = if parent_path == "/" {
                format!("/{}", name_str)
            } else {
                format!("{}/{}", parent_path, name_str)
            };

            let ops = match VaultOperations::new(&session) {
                Ok(o) => o,
                Err(_) => {
                    reply.error(libc::EIO);
                    return;
                }
            };

            let path = match VaultPath::parse(&child_path) {
                Ok(p) => p,
                Err(_) => {
                    reply.error(libc::EINVAL);
                    return;
                }
            };

            if let Err(e) = ops.delete_directory(&path).await {
                error!("Failed to delete directory: {}", e);
                reply.error(libc::EIO);
                return;
            }

            {
                let mut map = inodes.write().await;
                map.remove_inode(&child_path);
            }

            reply.ok();
        });
    }

    fn setattr(
        &mut self,
        _req: &Request,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        debug!("setattr: ino={}, size={:?}", ino, size);

        // For now, just return current attributes
        // TODO: Implement truncation if size is set
        self.getattr(_req, ino, reply);
    }
}
