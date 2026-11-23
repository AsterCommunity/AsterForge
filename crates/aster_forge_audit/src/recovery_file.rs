//! Private recovery publication; never includes driver errors or replays records.
//!
// TODO(AsterFS): replace descriptor-relative private directory/file creation, owner/mode/ACL
// validation and sync/rename publication with AsterFS primitives once they preserve these
// contracts on supported platforms. Keep the recovery envelope and no-replay policy in audit.

use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
pub(crate) fn write_private(path: &Path, payload: &[u8]) -> io::Result<PathBuf> {
    use rustix::fs::{self, AtFlags, Mode, OFlags};
    use std::fs::File;
    use std::io::Write;
    use std::path::Component;

    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = fs::open(
        if path.is_absolute() { "/" } else { "." },
        flags,
        Mode::empty(),
    )?;
    for component in path.components() {
        let name = match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => name,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "unsafe audit recovery path",
                ));
            }
        };
        match fs::mkdirat(&directory, name, Mode::from_raw_mode(0o700)) {
            Ok(()) => File::from(directory.try_clone()?).sync_all()?,
            Err(rustix::io::Errno::EXIST) => {}
            Err(error) => {
                return Err(io::Error::new(
                    io::Error::from(error).kind(),
                    format!("create recovery directory: {error}"),
                ));
            }
        }
        directory = fs::openat(&directory, name, flags, Mode::empty()).map_err(|error| {
            io::Error::new(
                io::Error::from(error).kind(),
                format!("open recovery directory: {error}"),
            )
        })?;
    }
    let stat = fs::fstat(&directory)?;
    if stat.st_uid != rustix::process::geteuid().as_raw() || stat.st_mode & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "audit recovery directory must be owner-only",
        ));
    }
    #[cfg(target_os = "macos")]
    verify_no_extended_acl(&directory).map_err(|error| {
        io::Error::new(error.kind(), format!("recovery directory ACL: {error}"))
    })?;
    let id = uuid::Uuid::new_v4();
    let name = format!("audit-{id}.json");
    let temporary = format!(".audit-{id}.tmp");
    let result = (|| {
        let create = || {
            fs::openat(
                &directory,
                temporary.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            )
        };
        let opened = create();
        #[cfg(target_os = "macos")]
        let opened = match opened {
            Err(rustix::io::Errno::NOENT) => create(),
            result => result,
        };
        let mut file = File::from(opened.map_err(|error| {
            io::Error::new(
                io::Error::from(error).kind(),
                format!("create recovery file: {error}"),
            )
        })?);
        #[cfg(target_os = "macos")]
        verify_no_extended_acl(&file)
            .map_err(|error| io::Error::new(error.kind(), format!("recovery file ACL: {error}")))?;
        file.write_all(payload)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        fs::renameat(&directory, temporary.as_str(), &directory, name.as_str())?;
        File::from(directory.try_clone()?).sync_all()?;
        Ok(path.join(&name))
    })();
    if result.is_err() {
        let _ = fs::unlinkat(&directory, temporary.as_str(), AtFlags::empty());
    }
    result
}

#[cfg(not(unix))]
pub(crate) fn write_private(_path: &Path, _payload: &[u8]) -> io::Result<PathBuf> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "private audit recovery requires a supported filesystem ACL policy",
    ))
}

// Darwin's extended ACLs can grant access independently of Unix mode bits. Reject every
// extended entry, including inherited entries, rather than claiming chmod removes those grants.
#[cfg(target_os = "macos")]
fn verify_no_extended_acl(file: &impl std::os::fd::AsRawFd) -> io::Result<()> {
    use std::ffi::{c_int, c_void};
    unsafe extern "C" {
        fn acl_get_fd_np(fd: c_int, acl_type: c_int) -> *mut c_void;
        fn acl_get_entry(acl: *mut c_void, entry_id: c_int, entry: *mut *mut c_void) -> c_int;
        fn acl_free(object: *mut c_void) -> c_int;
    }
    // SAFETY: file keeps the descriptor live; ACL_TYPE_EXTENDED is 0x100 in Darwin sys/acl.h.
    let acl = unsafe { acl_get_fd_np(file.as_raw_fd(), 0x100) };
    if acl.is_null() {
        let error = io::Error::last_os_error();
        // Darwin acl_get_fd_np delegates to filesec_get_property(FILESEC_ACL), which reports
        // ENOENT when this live descriptor has no extended ACL property.
        return if error.kind() == io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(error)
        };
    }
    let mut entry = std::ptr::null_mut();
    // SAFETY: acl is an owned live ACL from acl_get_fd_np; entry points to writable storage.
    let status = unsafe { acl_get_entry(acl, 0, &raw mut entry) };
    let error = io::Error::last_os_error();
    // SAFETY: this is the sole release of the ACL allocated by acl_get_fd_np.
    unsafe {
        acl_free(acl);
    }
    if status == -1 && error.raw_os_error() == Some(rustix::io::Errno::INVAL.raw_os_error()) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "extended ACL on audit recovery storage",
        ))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::write_private;
    use aster_forge_test::temp::TestTempDir;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn rejects_symlinks_and_existing_public_directories_without_chmod() {
        let root = TestTempDir::new("audit-path-policy");
        let root = root.path().canonicalize().unwrap();
        let public = root.join("public");
        std::fs::create_dir(&public).unwrap();
        std::fs::set_permissions(&public, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(write_private(&public, b"sensitive").is_err());
        assert_eq!(
            std::fs::metadata(&public).unwrap().permissions().mode() & 0o777,
            0o755
        );
        symlink(&public, root.join("alias")).unwrap();
        assert!(write_private(&root.join("alias"), b"sensitive").is_err());
        assert!(std::fs::read_dir(&public).unwrap().next().is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rejects_extended_acl_grants_even_with_owner_only_mode_bits() {
        let root = TestTempDir::new("audit-acl-policy");
        let private = root.path().canonicalize().unwrap().join("private");
        std::fs::create_dir(&private).unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
        let status = std::process::Command::new("/bin/chmod")
            .args(["+a", "everyone allow read,list"])
            .arg(&private)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(write_private(&private, b"sensitive").is_err());
        assert!(std::fs::read_dir(&private).unwrap().next().is_none());
    }
}
