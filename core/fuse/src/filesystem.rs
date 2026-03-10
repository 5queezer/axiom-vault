//! FUSE filesystem implementation for AxiomVault.
//!
//! Implements the fuser::Filesystem trait to expose an encrypted vault
//! as a standard filesystem.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use fuser::{
    BsdFileFlags, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation,
    INodeNo, LockOwner, OpenFlags, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow, WriteFlags,
};
use tokio::runtime::Handle;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

use axiomvault_common::VaultPath;
use axiomvault_vault::{VaultOperations, VaultSession};

/// Helper function to create FileAttr with common defaults.
fn create_file_attr(ino: INodeNo, is_dir: bool, size: u64) -> FileAttr {
    let now = SystemTime::now();
    FileAttr {
        ino,
        size,
        blocks: size.div_ceil(512),
        atime: now,
        mtime: now,
        ctime: now,
        crtime: now,
        kind: if is_dir {
            FileType::Directory
        } else {
            FileType::RegularFile
        },
        perm: if is_dir { 0o700 } else { 0o600 },
        nlink: if is_dir { 2 } else { 1 },
        uid: unsafe { libc::getuid() },
        gid: unsafe { libc::getgid() },
        rdev: 0,
        blksize: 4096,
        flags: 0,
    }
}

/// Inode number mapping to vault paths.
struct InodeMap {
    path_to_inode: HashMap<String, INodeNo>,
    inode_to_path: HashMap<INodeNo, String>,
    next_inode: u64,
}

impl InodeMap {
    /// Return the inode for a path, if it has been assigned.
    fn get_inode_for_path(&self, path: &str) -> Option<INodeNo> {
        self.path_to_inode.get(path).copied()
    }
}

impl InodeMap {
    fn new() -> Self {
        let mut map = Self {
            path_to_inode: HashMap::new(),
            inode_to_path: HashMap::new(),
            next_inode: 2, // 1 is reserved for root
        };
        // Root inode
        map.path_to_inode.insert("/".to_string(), INodeNo::ROOT);
        map.inode_to_path.insert(INodeNo::ROOT, "/".to_string());
        map
    }

    fn get_or_create_inode(&mut self, path: &str) -> INodeNo {
        if let Some(&ino) = self.path_to_inode.get(path) {
            ino
        } else {
            let ino = INodeNo(self.next_inode);
            self.next_inode += 1;
            self.path_to_inode.insert(path.to_string(), ino);
            self.inode_to_path.insert(ino, path.to_string());
            ino
        }
    }

    fn get_path(&self, inode: INodeNo) -> Option<&str> {
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
    open_files: Arc<RwLock<HashMap<FileHandle, OpenFile>>>,
    next_fh: Arc<RwLock<u64>>,
    ttl: Duration,
}

// SAFETY: All components are Arc/RwLock (thread-safe) or owned Tokio Handle.
// No raw pointers or thread-unsafe data structures are stored.
unsafe impl Send for VaultFilesystem {}

// SAFETY: All mutable state is protected by RwLock, ensuring safe concurrent access.
unsafe impl Sync for VaultFilesystem {}

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
}

impl Filesystem for VaultFilesystem {
    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };

        debug!("lookup: parent={}", parent);

        let session = self.session.clone();
        let inodes = self.inodes.clone();
        let ttl = self.ttl;

        self.runtime.block_on(async move {
            let parent_path = {
                let map = inodes.read().await;
                match map.get_path(parent) {
                    Some(p) => p.to_string(),
                    None => {
                        reply.error(Errno::ENOENT);
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
                    reply.error(Errno::EIO);
                    return;
                }
            };

            let path = match VaultPath::parse(&child_path) {
                Ok(p) => p,
                Err(_) => {
                    reply.error(Errno::ENOENT);
                    return;
                }
            };

            if !ops.exists(&path).await {
                reply.error(Errno::ENOENT);
                return;
            }

            match ops.metadata(&path).await {
                Ok((_, is_dir, size)) => {
                    let mut map = inodes.write().await;
                    let ino = map.get_or_create_inode(&child_path);
                    let attr = create_file_attr(ino, is_dir, size.unwrap_or(0));
                    reply.entry(&ttl, &attr, Generation(0));
                }
                Err(e) => {
                    error!("Failed to get metadata: {}", e);
                    reply.error(Errno::EIO);
                }
            }
        });
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
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
                        reply.error(Errno::ENOENT);
                        return;
                    }
                }
            };

            if path_str == "/" {
                // Root directory
                let attr = create_file_attr(INodeNo::ROOT, true, 0);
                reply.attr(&ttl, &attr);
                return;
            }

            let ops = match VaultOperations::new(&session) {
                Ok(o) => o,
                Err(_) => {
                    reply.error(Errno::EIO);
                    return;
                }
            };

            let path = match VaultPath::parse(&path_str) {
                Ok(p) => p,
                Err(_) => {
                    reply.error(Errno::ENOENT);
                    return;
                }
            };

            match ops.metadata(&path).await {
                Ok((_, is_dir, size)) => {
                    let attr = create_file_attr(ino, is_dir, size.unwrap_or(0));
                    reply.attr(&ttl, &attr);
                }
                Err(e) => {
                    error!("Failed to get metadata: {}", e);
                    reply.error(Errno::EIO);
                }
            }
        });
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        debug!("readdir: ino={}, offset={}", u64::from(ino), offset);

        let session = self.session.clone();
        let inodes = self.inodes.clone();

        self.runtime.block_on(async move {
            let path_str = {
                let map = inodes.read().await;
                match map.get_path(ino) {
                    Some(p) => p.to_string(),
                    None => {
                        reply.error(Errno::ENOENT);
                        return;
                    }
                }
            };

            let ops = match VaultOperations::new(&session) {
                Ok(o) => o,
                Err(_) => {
                    reply.error(Errno::EIO);
                    return;
                }
            };

            let path = match VaultPath::parse(&path_str) {
                Ok(p) => p,
                Err(_) => {
                    reply.error(Errno::ENOENT);
                    return;
                }
            };

            let entries = match ops.list_directory(&path).await {
                Ok(e) => e,
                Err(e) => {
                    error!("Failed to list directory: {}", e);
                    reply.error(Errno::EIO);
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
                // Look up the actual parent inode instead of always returning 1.
                let parent_ino = {
                    let map = inodes.read().await;
                    if path_str == "/" {
                        // Root's parent is itself (POSIX convention).
                        INodeNo::ROOT
                    } else {
                        // Walk up one component to find the parent path string.
                        let parent_path = path_str
                            .rfind('/')
                            .map(|idx| {
                                let p = &path_str[..idx];
                                if p.is_empty() {
                                    "/"
                                } else {
                                    p
                                }
                            })
                            .unwrap_or("/");
                        map.get_inode_for_path(parent_path).unwrap_or(INodeNo::ROOT)
                    }
                };
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

                if reply.add(child_ino, (idx + 3) as u64, file_type, name) {
                    break;
                }
            }

            reply.ok();
        });
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        debug!("open: ino={}, flags={:?}", ino, flags);

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
                        reply.error(Errno::ENOENT);
                        return;
                    }
                }
            };

            let ops = match VaultOperations::new(&session) {
                Ok(o) => o,
                Err(_) => {
                    reply.error(Errno::EIO);
                    return;
                }
            };

            let path = match VaultPath::parse(&path_str) {
                Ok(p) => p,
                Err(_) => {
                    reply.error(Errno::ENOENT);
                    return;
                }
            };

            // Read file content into buffer
            let buffer = match ops.read_file(&path).await {
                Ok(data) => data,
                Err(e) => {
                    error!("Failed to read file: {}", e);
                    reply.error(Errno::EIO);
                    return;
                }
            };

            let fh = {
                let mut fh_guard = next_fh.write().await;
                let handle = FileHandle(*fh_guard);
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

            reply.opened(fh, FopenFlags::empty());
        });
    }

    fn read(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        debug!("read: fh={}, offset={}, size={}", u64::from(fh), offset, size);

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
                    reply.error(Errno::EBADF);
                }
            }
        });
    }

    fn write(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        debug!("write: fh={}, offset={}, size={}", u64::from(fh), offset, data.len());

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
                    reply.error(Errno::EBADF);
                }
            }
        });
    }

    fn release(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        debug!("release: fh={}", u64::from(fh));

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
                            reply.error(Errno::EIO);
                            return;
                        }
                    };

                    let path = match VaultPath::parse(&file.path) {
                        Ok(p) => p,
                        Err(e) => {
                            error!("Invalid path: {}", e);
                            reply.error(Errno::EIO);
                            return;
                        }
                    };

                    if let Err(e) = ops.update_file(&path, &file.buffer).await {
                        error!("Failed to write file: {}", e);
                        reply.error(Errno::EIO);
                        return;
                    }

                    info!("File saved");
                }
            }

            reply.ok();
        });
    }

    fn create(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(Errno::EINVAL);
                return;
            }
        };

        debug!("create: parent={}", u64::from(parent));

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
                        reply.error(Errno::ENOENT);
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
                    reply.error(Errno::EIO);
                    return;
                }
            };

            let path = match VaultPath::parse(&child_path) {
                Ok(p) => p,
                Err(_) => {
                    reply.error(Errno::EINVAL);
                    return;
                }
            };

            // Create empty file
            if let Err(e) = ops.create_file(&path, &[]).await {
                error!("Failed to create file: {}", e);
                reply.error(Errno::EIO);
                return;
            }

            let ino = {
                let mut map = inodes.write().await;
                map.get_or_create_inode(&child_path)
            };

            let fh = {
                let mut fh_guard = next_fh.write().await;
                let handle = FileHandle(*fh_guard);
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

            let attr = create_file_attr(ino, false, 0);

            reply.created(&ttl, &attr, Generation(0), fh, FopenFlags::empty());
        });
    }

    fn mkdir(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(Errno::EINVAL);
                return;
            }
        };

        debug!("mkdir: parent={}", u64::from(parent));

        let session = self.session.clone();
        let inodes = self.inodes.clone();
        let ttl = self.ttl;

        self.runtime.block_on(async move {
            let parent_path = {
                let map = inodes.read().await;
                match map.get_path(parent) {
                    Some(p) => p.to_string(),
                    None => {
                        reply.error(Errno::ENOENT);
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
                    reply.error(Errno::EIO);
                    return;
                }
            };

            let path = match VaultPath::parse(&child_path) {
                Ok(p) => p,
                Err(_) => {
                    reply.error(Errno::EINVAL);
                    return;
                }
            };

            if let Err(e) = ops.create_directory(&path).await {
                error!("Failed to create directory: {}", e);
                reply.error(Errno::EIO);
                return;
            }

            let ino = {
                let mut map = inodes.write().await;
                map.get_or_create_inode(&child_path)
            };

            let attr = create_file_attr(ino, true, 0);

            reply.entry(&ttl, &attr, Generation(0));
        });
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(Errno::EINVAL);
                return;
            }
        };

        debug!("unlink: parent={}", parent);

        let session = self.session.clone();
        let inodes = self.inodes.clone();

        self.runtime.block_on(async move {
            let parent_path = {
                let map = inodes.read().await;
                match map.get_path(parent) {
                    Some(p) => p.to_string(),
                    None => {
                        reply.error(Errno::ENOENT);
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
                    reply.error(Errno::EIO);
                    return;
                }
            };

            let path = match VaultPath::parse(&child_path) {
                Ok(p) => p,
                Err(_) => {
                    reply.error(Errno::EINVAL);
                    return;
                }
            };

            if let Err(e) = ops.delete_file(&path).await {
                error!("Failed to delete file: {}", e);
                reply.error(Errno::EIO);
                return;
            }

            {
                let mut map = inodes.write().await;
                map.remove_inode(&child_path);
            }

            reply.ok();
        });
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(Errno::EINVAL);
                return;
            }
        };

        debug!("rmdir: parent={}", parent);

        let session = self.session.clone();
        let inodes = self.inodes.clone();

        self.runtime.block_on(async move {
            let parent_path = {
                let map = inodes.read().await;
                match map.get_path(parent) {
                    Some(p) => p.to_string(),
                    None => {
                        reply.error(Errno::ENOENT);
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
                    reply.error(Errno::EIO);
                    return;
                }
            };

            let path = match VaultPath::parse(&child_path) {
                Ok(p) => p,
                Err(_) => {
                    reply.error(Errno::EINVAL);
                    return;
                }
            };

            if let Err(e) = ops.delete_directory(&path).await {
                error!("Failed to delete directory: {}", e);
                reply.error(Errno::EIO);
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
        &self,
        _req: &Request,
        ino: INodeNo,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        fh: Option<FileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        debug!("setattr: ino={}, size={:?}", u64::from(ino), size);

        // For now, just return current attributes
        // TODO: Implement truncation if size is set
        self.getattr(_req, ino, fh, reply);
    }
}
