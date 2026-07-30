//! Native `fuser` mapping for read-only, writable, and durable-create Linux engines.

use std::{
    ffi::OsStr,
    io,
    path::Path,
    time::{Duration, SystemTime},
};

use aster_forge_cloud_files_core::CloudFilesBackend;
use fuser::{
    BsdFileFlags, Config, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags,
    Generation, INodeNo, LockOwner, MountOption, OpenAccMode, OpenFlags, RenameFlags, ReplyAttr,
    ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request,
    TimeOrNow, WriteFlags,
};
use tokio::runtime::Handle;

use crate::{
    LinuxDirectoryHandle, LinuxDispatchRejection, LinuxErrorCode, LinuxFileAccess,
    LinuxFileAttributes, LinuxFileHandle, LinuxInode, LinuxInvalidation, LinuxMountConfig,
    LinuxNode, LinuxNodeKind, LinuxReadOnlyEngine, LinuxRequestDispatcher, LinuxWritableEngine,
    LinuxWritebackStore, Result,
};

/// Native kernel-cache invalidation port owned by one mounted FUSE session.
#[derive(Debug, Clone)]
pub struct LinuxKernelNotifier {
    inner: fuser::Notifier,
}

impl LinuxKernelNotifier {
    fn new(inner: fuser::Notifier) -> Self {
        Self { inner }
    }

    /// Applies one engine-produced invalidation to the mounted kernel namespace.
    pub fn apply(&self, invalidation: &LinuxInvalidation) -> io::Result<()> {
        match invalidation {
            LinuxInvalidation::Entry { parent, name } => self
                .inner
                .inval_entry(INodeNo(parent.get()), OsStr::new(name)),
            LinuxInvalidation::Inode { inode } => {
                self.inner.inval_inode(INodeNo(inode.get()), 0, 0)
            }
            LinuxInvalidation::Delete {
                parent,
                child,
                name,
            } => self.inner.delete(
                INodeNo(parent.get()),
                INodeNo(child.get()),
                OsStr::new(name),
            ),
        }
    }

    /// Applies an ordered engine plan, stopping at the first kernel error.
    pub fn apply_all<'a>(
        &self,
        invalidations: impl IntoIterator<Item = &'a LinuxInvalidation>,
    ) -> io::Result<()> {
        for invalidation in invalidations {
            self.apply(invalidation)?;
        }
        Ok(())
    }
}

/// Background FUSE session that exposes notifier and explicit join/unmount boundaries.
#[derive(Debug)]
pub struct LinuxBackgroundSession {
    inner: fuser::BackgroundSession,
}

impl LinuxBackgroundSession {
    /// Returns a cloneable kernel notification port for remote-change workers.
    pub fn notifier(&self) -> LinuxKernelNotifier {
        LinuxKernelNotifier::new(self.inner.notifier())
    }

    /// Waits for the mounted session thread to finish.
    pub fn join(self) -> io::Result<()> {
        self.inner.join()
    }

    /// Unmounts the filesystem and then joins its session thread.
    pub fn unmount_and_join(self) -> io::Result<()> {
        self.inner.umount_and_join()
    }
}

fn errno(code: LinuxErrorCode) -> Errno {
    match code {
        LinuxErrorCode::NotFound => Errno::ENOENT,
        LinuxErrorCode::AlreadyExists => Errno::EEXIST,
        LinuxErrorCode::DirectoryNotEmpty => Errno::ENOTEMPTY,
        LinuxErrorCode::IsDirectory => Errno::EISDIR,
        LinuxErrorCode::NotDirectory => Errno::ENOTDIR,
        LinuxErrorCode::AccessDenied => Errno::EACCES,
        LinuxErrorCode::ReadOnlyFilesystem => Errno::EROFS,
        LinuxErrorCode::TryAgain => Errno::EAGAIN,
        LinuxErrorCode::Stale => Errno::ESTALE,
        LinuxErrorCode::NotSupported => Errno::ENOSYS,
        LinuxErrorCode::InvalidArgument => Errno::EINVAL,
        LinuxErrorCode::Io => Errno::EIO,
    }
}

fn file_access(mode: OpenAccMode) -> LinuxFileAccess {
    match mode {
        OpenAccMode::O_RDONLY => LinuxFileAccess::Read,
        OpenAccMode::O_WRONLY => LinuxFileAccess::Write,
        OpenAccMode::O_RDWR => LinuxFileAccess::ReadWrite,
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

/// Native writable FUSE filesystem with direct-I/O writeback and optional durable create.
pub struct LinuxWritableFilesystem<B, S> {
    engine: LinuxWritableEngine<B, S>,
    dispatcher: LinuxRequestDispatcher,
}

impl<B, S> LinuxWritableFilesystem<B, S>
where
    B: CloudFilesBackend + 'static,
    S: LinuxWritebackStore + 'static,
{
    /// Creates a native writable filesystem without mounting it.
    pub fn new(
        engine: LinuxWritableEngine<B, S>,
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

impl<B, S> Filesystem for LinuxWritableFilesystem<B, S>
where
    B: CloudFilesBackend + 'static,
    S: LinuxWritebackStore + 'static,
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
        let ttl = engine.readonly().attribute_policy().cache_ttl();
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
        handle: Option<FileHandle>,
        reply: ReplyAttr,
    ) {
        let inode = match request_inode(inode) {
            Ok(inode) => inode,
            Err(error) => {
                reply.error(errno(error.error_code()));
                return;
            }
        };
        let handle = match handle
            .map(|handle| LinuxFileHandle::new(u64::from(handle)))
            .transpose()
        {
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
        let ttl = engine.readonly().attribute_policy().cache_ttl();
        reservation.spawn(async move {
            match engine.getattr(inode, handle).await {
                Ok(node) => reply.attr(&ttl, &file_attr(node.attributes())),
                Err(error) => reply.error(errno(error.error_code())),
            }
        });
    }

    fn setattr(
        &self,
        _request: &Request,
        inode: INodeNo,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        ctime: Option<SystemTime>,
        handle: Option<FileHandle>,
        crtime: Option<SystemTime>,
        chgtime: Option<SystemTime>,
        bkuptime: Option<SystemTime>,
        flags: Option<BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        if mode.is_some()
            || uid.is_some()
            || gid.is_some()
            || atime.is_some()
            || mtime.is_some()
            || ctime.is_some()
            || crtime.is_some()
            || chgtime.is_some()
            || bkuptime.is_some()
            || flags.is_some()
        {
            reply.error(Errno::ENOSYS);
            return;
        }
        let Some(size) = size else {
            reply.error(Errno::EINVAL);
            return;
        };
        let inode = match request_inode(inode) {
            Ok(inode) => inode,
            Err(error) => {
                reply.error(errno(error.error_code()));
                return;
            }
        };
        let handle = match handle
            .map(|handle| LinuxFileHandle::new(u64::from(handle)))
            .transpose()
        {
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
        let ttl = engine.readonly().attribute_policy().cache_ttl();
        reservation.spawn(async move {
            let result = match handle {
                Some(handle) => engine.truncate_file(inode, handle, size).await,
                None => engine.truncate_once(inode, size).await,
            };
            match result {
                Ok(node) => reply.attr(&ttl, &file_attr(node.attributes())),
                Err(error) => reply.error(errno(error.error_code())),
            }
        });
    }

    fn open(&self, _request: &Request, inode: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
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
        let access = file_access(flags.acc_mode());
        reservation.spawn(async move {
            match engine.open_file(inode, access).await {
                Ok(handle) => reply.opened(FileHandle(handle.get()), FopenFlags::FOPEN_DIRECT_IO),
                Err(error) => reply.error(errno(error.error_code())),
            }
        });
    }

    fn create(
        &self,
        _request: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
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
        let access = file_access(OpenFlags(flags).acc_mode());
        let ttl = engine.readonly().attribute_policy().cache_ttl();
        reservation.spawn(async move {
            match engine.create_file(parent, &name, mode, umask, access).await {
                Ok((node, handle)) => reply.created(
                    &ttl,
                    &file_attr(node.attributes()),
                    Generation(node.generation().get()),
                    FileHandle(handle.get()),
                    FopenFlags::FOPEN_DIRECT_IO,
                ),
                Err(error) => reply.error(errno(error.error_code())),
            }
        });
    }

    fn mkdir(
        &self,
        _request: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
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
        let ttl = engine.readonly().attribute_policy().cache_ttl();
        reservation.spawn(async move {
            match engine.create_directory(parent, &name, mode, umask).await {
                Ok(node) => reply_entry(reply, ttl, &node),
                Err(error) => reply.error(errno(error.error_code())),
            }
        });
    }

    fn unlink(&self, _request: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        self.remove_request(parent, name, LinuxNodeKind::File, reply);
    }

    fn rmdir(&self, _request: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        self.remove_request(parent, name, LinuxNodeKind::Directory, reply);
    }

    fn rename(
        &self,
        _request: &Request,
        parent: INodeNo,
        name: &OsStr,
        new_parent: INodeNo,
        new_name: &OsStr,
        flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        if flags.intersects(RenameFlags::RENAME_EXCHANGE | RenameFlags::RENAME_WHITEOUT) {
            reply.error(Errno::ENOSYS);
            return;
        }
        let result = request_inode(parent)
            .and_then(|parent| request_inode(new_parent).map(|new_parent| (parent, new_parent)));
        let (parent, new_parent) = match result {
            Ok(result) => result,
            Err(error) => {
                reply.error(errno(error.error_code()));
                return;
            }
        };
        let (Some(name), Some(new_name)) = (
            name.to_str().map(str::to_owned),
            new_name.to_str().map(str::to_owned),
        ) else {
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
        let no_replace = flags.contains(RenameFlags::RENAME_NOREPLACE);
        reservation.spawn(async move {
            match engine
                .rename(parent, &name, new_parent, &new_name, no_replace)
                .await
            {
                Ok(()) => reply.ok(),
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
        let result = request_inode(inode).and_then(|inode| {
            LinuxFileHandle::new(u64::from(handle)).map(|handle| (inode, handle))
        });
        let (inode, handle) = match result {
            Ok(result) => result,
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
        inode: INodeNo,
        handle: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        let result = request_inode(inode).and_then(|inode| {
            LinuxFileHandle::new(u64::from(handle)).map(|handle| (inode, handle))
        });
        let (inode, handle) = match result {
            Ok(result) => result,
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
        let bytes = bytes::Bytes::copy_from_slice(data);
        reservation.spawn(async move {
            match engine.write_file(inode, handle, offset, bytes).await {
                Ok(written) => reply.written(written),
                Err(error) => reply.error(errno(error.error_code())),
            }
        });
    }

    fn flush(
        &self,
        _request: &Request,
        inode: INodeNo,
        handle: FileHandle,
        _lock_owner: LockOwner,
        reply: ReplyEmpty,
    ) {
        self.sync_request(inode, handle, false, reply);
    }

    fn fsync(
        &self,
        _request: &Request,
        inode: INodeNo,
        handle: FileHandle,
        data_only: bool,
        reply: ReplyEmpty,
    ) {
        self.sync_request(inode, handle, data_only, reply);
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
        let handle = match LinuxFileHandle::new(u64::from(handle)) {
            Ok(handle) => handle,
            Err(_) => {
                reply.ok();
                return;
            }
        };
        let engine = self.engine.clone();
        self.dispatcher.spawn_cleanup(async move {
            let _result = engine.release_file(handle).await;
            reply.ok();
        });
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

impl<B, S> LinuxWritableFilesystem<B, S>
where
    B: CloudFilesBackend + 'static,
    S: LinuxWritebackStore + 'static,
{
    fn sync_request(&self, inode: INodeNo, handle: FileHandle, data_only: bool, reply: ReplyEmpty) {
        let result = request_inode(inode).and_then(|inode| {
            LinuxFileHandle::new(u64::from(handle)).map(|handle| (inode, handle))
        });
        let (inode, handle) = match result {
            Ok(result) => result,
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
            match engine.sync_file(inode, handle, data_only).await {
                Ok(()) => reply.ok(),
                Err(error) => reply.error(errno(error.error_code())),
            }
        });
    }

    fn remove_request(
        &self,
        parent: INodeNo,
        name: &OsStr,
        kind: LinuxNodeKind,
        reply: ReplyEmpty,
    ) {
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
        reservation.spawn(async move {
            match engine.remove(parent, &name, kind).await {
                Ok(()) => reply.ok(),
                Err(error) => reply.error(errno(error.error_code())),
            }
        });
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

/// Spawns a read-only FUSE session and returns its kernel invalidation lifecycle handle.
pub fn spawn_mount_read_only<B>(
    filesystem: LinuxReadOnlyFilesystem<B>,
    mountpoint: impl AsRef<Path>,
    config: &LinuxMountConfig,
) -> io::Result<LinuxBackgroundSession>
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
    fuser::spawn_mount(filesystem, mountpoint, &options)
        .map(|inner| LinuxBackgroundSession { inner })
}

/// Mounts a blocking writable FUSE session without kernel writeback-cache semantics.
pub fn mount_writable<B, S>(
    filesystem: LinuxWritableFilesystem<B, S>,
    mountpoint: impl AsRef<Path>,
    config: &LinuxMountConfig,
) -> io::Result<()>
where
    B: CloudFilesBackend + 'static,
    S: LinuxWritebackStore + 'static,
{
    let mut options = Config::default();
    options.n_threads = config.kernel_threads();
    options.clone_fd = config.clone_fd();
    options.mount_options.extend([
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

/// Spawns a writable direct-I/O FUSE session with a kernel invalidation handle.
pub fn spawn_mount_writable<B, S>(
    filesystem: LinuxWritableFilesystem<B, S>,
    mountpoint: impl AsRef<Path>,
    config: &LinuxMountConfig,
) -> io::Result<LinuxBackgroundSession>
where
    B: CloudFilesBackend + 'static,
    S: LinuxWritebackStore + 'static,
{
    let mut options = Config::default();
    options.n_threads = config.kernel_threads();
    options.clone_fd = config.clone_fd();
    options.mount_options.extend([
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
    fuser::spawn_mount(filesystem, mountpoint, &options)
        .map(|inner| LinuxBackgroundSession { inner })
}
