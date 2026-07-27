//! Native `fuser` mapping for the read-only Linux engine.

use std::{ffi::OsStr, io, path::Path, time::Duration};

use aster_forge_cloud_files_core::CloudFilesBackend;
use fuser::{
    Config, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation, INodeNo,
    LockOwner, MountOption, OpenAccMode, OpenFlags, ReplyAttr, ReplyData, ReplyDirectory,
    ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request, WriteFlags,
};
use tokio::runtime::Handle;

use crate::{
    LinuxDirectoryHandle, LinuxDispatchRejection, LinuxErrorCode, LinuxFileAttributes,
    LinuxFileHandle, LinuxInode, LinuxMountConfig, LinuxNode, LinuxNodeKind, LinuxReadOnlyEngine,
    LinuxRequestDispatcher, Result,
};

fn errno(code: LinuxErrorCode) -> Errno {
    match code {
        LinuxErrorCode::NotFound => Errno::ENOENT,
        LinuxErrorCode::AccessDenied => Errno::EACCES,
        LinuxErrorCode::ReadOnlyFilesystem => Errno::EROFS,
        LinuxErrorCode::TryAgain => Errno::EAGAIN,
        LinuxErrorCode::Stale => Errno::ESTALE,
        LinuxErrorCode::NotSupported => Errno::ENOSYS,
        LinuxErrorCode::InvalidArgument => Errno::EINVAL,
        LinuxErrorCode::Io => Errno::EIO,
    }
}

fn request_inode(inode: INodeNo) -> Result<LinuxInode> {
    LinuxInode::new(u64::from(inode))
}

fn file_type(kind: LinuxNodeKind) -> FileType {
    match kind {
        LinuxNodeKind::File => FileType::RegularFile,
        LinuxNodeKind::Directory => FileType::Directory,
    }
}

fn file_attr(attributes: &LinuxFileAttributes) -> FileAttr {
    FileAttr {
        ino: INodeNo(attributes.inode().get()),
        size: attributes.size(),
        blocks: attributes.blocks(),
        atime: attributes.time(),
        mtime: attributes.time(),
        ctime: attributes.time(),
        crtime: attributes.time(),
        kind: file_type(attributes.kind()),
        perm: attributes.permissions(),
        nlink: attributes.links(),
        uid: attributes.uid(),
        gid: attributes.gid(),
        rdev: 0,
        blksize: attributes.block_size(),
        flags: 0,
    }
}

fn reply_entry(reply: ReplyEntry, ttl: Duration, node: &LinuxNode) {
    reply.entry(
        &ttl,
        &file_attr(node.attributes()),
        Generation(node.generation().get()),
    );
}

fn rejection_errno(rejection: LinuxDispatchRejection) -> Errno {
    errno(rejection.error_code())
}

/// Native FUSE filesystem that delegates all remote work to a bounded explicit runtime.
pub struct LinuxReadOnlyFilesystem<B> {
    engine: LinuxReadOnlyEngine<B>,
    dispatcher: LinuxRequestDispatcher,
}

impl<B> LinuxReadOnlyFilesystem<B>
where
    B: CloudFilesBackend + 'static,
{
    /// Creates a native filesystem without mounting it.
    pub fn new(
        engine: LinuxReadOnlyEngine<B>,
        runtime: Handle,
        max_in_flight: usize,
    ) -> Result<Self> {
        Ok(Self {
            engine,
            dispatcher: LinuxRequestDispatcher::new(runtime, max_in_flight)?,
        })
    }

    /// Returns the shared dispatcher for metrics and controlled shutdown.
    pub const fn dispatcher(&self) -> &LinuxRequestDispatcher {
        &self.dispatcher
    }
}

impl<B> Filesystem for LinuxReadOnlyFilesystem<B>
where
    B: CloudFilesBackend + 'static,
{
    fn destroy(&mut self) {
        self.dispatcher.close();
    }

    fn lookup(&self, _request: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let parent = match request_inode(parent) {
            Ok(parent) => parent,
            Err(error) => {
                reply.error(errno(error.error_code()));
                return;
            }
        };
        let Some(name) = name.to_str().map(str::to_owned) else {
            reply.error(Errno::EINVAL);
            return;
        };
        let reservation = match self.dispatcher.reserve() {
            Ok(reservation) => reservation,
            Err(rejection) => {
                reply.error(rejection_errno(rejection));
                return;
            }
        };
        let engine = self.engine.clone();
        let ttl = engine.attribute_policy().cache_ttl();
        reservation.spawn(async move {
            match engine.lookup(parent, &name).await {
                Ok(node) => reply_entry(reply, ttl, &node),
                Err(error) => reply.error(errno(error.error_code())),
            }
        });
    }

    fn getattr(
        &self,
        _request: &Request,
        inode: INodeNo,
        _handle: Option<FileHandle>,
        reply: ReplyAttr,
    ) {
        let inode = match request_inode(inode) {
            Ok(inode) => inode,
            Err(error) => {
                reply.error(errno(error.error_code()));
                return;
            }
        };
        let reservation = match self.dispatcher.reserve() {
            Ok(reservation) => reservation,
            Err(rejection) => {
                reply.error(rejection_errno(rejection));
                return;
            }
        };
        let engine = self.engine.clone();
        let ttl = engine.attribute_policy().cache_ttl();
        reservation.spawn(async move {
            match engine.getattr(inode).await {
                Ok(node) => reply.attr(&ttl, &file_attr(node.attributes())),
                Err(error) => reply.error(errno(error.error_code())),
            }
        });
    }

    fn open(&self, _request: &Request, inode: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        if flags.acc_mode() != OpenAccMode::O_RDONLY {
            reply.error(Errno::EROFS);
            return;
        }
        let inode = match request_inode(inode) {
            Ok(inode) => inode,
            Err(error) => {
                reply.error(errno(error.error_code()));
                return;
            }
        };
        let reservation = match self.dispatcher.reserve() {
            Ok(reservation) => reservation,
            Err(rejection) => {
                reply.error(rejection_errno(rejection));
                return;
            }
        };
        let engine = self.engine.clone();
        reservation.spawn(async move {
            match engine.open_file(inode).await {
                Ok(handle) => reply.opened(
                    FileHandle(handle.get()),
                    FopenFlags::FOPEN_DIRECT_IO | FopenFlags::FOPEN_NOFLUSH,
                ),
                Err(error) => reply.error(errno(error.error_code())),
            }
        });
    }

    fn read(
        &self,
        _request: &Request,
        inode: INodeNo,
        handle: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let inode = match request_inode(inode) {
            Ok(inode) => inode,
            Err(error) => {
                reply.error(errno(error.error_code()));
                return;
            }
        };
        let handle = match LinuxFileHandle::new(u64::from(handle)) {
            Ok(handle) => handle,
            Err(error) => {
                reply.error(errno(error.error_code()));
                return;
            }
        };
        let reservation = match self.dispatcher.reserve() {
            Ok(reservation) => reservation,
            Err(rejection) => {
                reply.error(rejection_errno(rejection));
                return;
            }
        };
        let engine = self.engine.clone();
        reservation.spawn(async move {
            match engine.read_file(inode, handle, offset, size).await {
                Ok(bytes) => reply.data(&bytes),
                Err(error) => reply.error(errno(error.error_code())),
            }
        });
    }

    fn write(
        &self,
        _request: &Request,
        _inode: INodeNo,
        _handle: FileHandle,
        _offset: u64,
        _data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        reply.error(Errno::EROFS);
    }

    fn release(
        &self,
        _request: &Request,
        _inode: INodeNo,
        handle: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        if let Ok(handle) = LinuxFileHandle::new(u64::from(handle)) {
            let _result = self.engine.release_file(handle);
        }
        reply.ok();
    }

    fn opendir(&self, _request: &Request, inode: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        let inode = match request_inode(inode) {
            Ok(inode) => inode,
            Err(error) => {
                reply.error(errno(error.error_code()));
                return;
            }
        };
        let reservation = match self.dispatcher.reserve() {
            Ok(reservation) => reservation,
            Err(rejection) => {
                reply.error(rejection_errno(rejection));
                return;
            }
        };
        let engine = self.engine.clone();
        reservation.spawn(async move {
            match engine.open_directory(inode).await {
                Ok(handle) => reply.opened(FileHandle(handle.get()), FopenFlags::FOPEN_CACHE_DIR),
                Err(error) => reply.error(errno(error.error_code())),
            }
        });
    }

    fn readdir(
        &self,
        _request: &Request,
        inode: INodeNo,
        handle: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let result = request_inode(inode).and_then(|inode| {
            let handle = LinuxDirectoryHandle::new(u64::from(handle))?;
            self.engine
                .directory_snapshot(inode, handle)
                .map(|snapshot| (inode, snapshot))
        });
        let (inode, snapshot) = match result {
            Ok(result) => result,
            Err(error) => {
                reply.error(errno(error.error_code()));
                return;
            }
        };
        let start = match usize::try_from(offset) {
            Ok(start) => start,
            Err(_) => {
                reply.ok();
                return;
            }
        };
        let mut index = 0usize;
        let mut full = false;
        if index >= start {
            full = reply.add(INodeNo(inode.get()), 1, FileType::Directory, ".");
        }
        index += 1;
        if !full && index >= start {
            full = reply.add(
                INodeNo(snapshot.parent().get()),
                2,
                FileType::Directory,
                "..",
            );
        }
        index += 1;
        for entry in snapshot.entries() {
            if full {
                break;
            }
            let next = match u64::try_from(index + 1) {
                Ok(next) => next,
                Err(_) => break,
            };
            if index >= start {
                full = reply.add(
                    INodeNo(entry.inode().get()),
                    next,
                    file_type(entry.kind()),
                    entry.name(),
                );
            }
            index += 1;
        }
        reply.ok();
    }

    fn releasedir(
        &self,
        _request: &Request,
        _inode: INodeNo,
        handle: FileHandle,
        _flags: OpenFlags,
        reply: ReplyEmpty,
    ) {
        if let Ok(handle) = LinuxDirectoryHandle::new(u64::from(handle)) {
            let _result = self.engine.release_directory(handle);
        }
        reply.ok();
    }
}

/// Mounts a blocking read-only FUSE session using product-neutral hardened defaults.
pub fn mount_read_only<B>(
    filesystem: LinuxReadOnlyFilesystem<B>,
    mountpoint: impl AsRef<Path>,
    config: &LinuxMountConfig,
) -> io::Result<()>
where
    B: CloudFilesBackend + 'static,
{
    let mut options = Config::default();
    options.n_threads = config.kernel_threads();
    options.clone_fd = config.clone_fd();
    options.mount_options.extend([
        MountOption::RO,
        MountOption::DefaultPermissions,
        MountOption::NoDev,
        MountOption::NoSuid,
        MountOption::NoExec,
        MountOption::FSName(config.filesystem_name().to_owned()),
    ]);
    if let Some(subtype) = config.subtype() {
        options
            .mount_options
            .push(MountOption::Subtype(subtype.to_owned()));
    }
    fuser::mount(filesystem, mountpoint, &options)
}
