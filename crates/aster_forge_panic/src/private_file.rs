use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;

use crate::ReportWriter;

/// Preopened private report file with nonblocking lock acquisition.
///
/// Linux: walks every path component using directory descriptors and NOFOLLOW,
/// creates missing directories with 0700 and files with 0600, rejects parent
/// traversal, links, special files, foreign ownership and group/other file access.
/// Existing files are never chmodded or truncated. Ancestors must be managed by
/// the application/OS; this does not defend against an attacker with the same UID.
/// Other platforms (including macOS): fail closed; inject a writer enforcing the product's ACL policy.
/// Open failures are retained as unavailable; no file IO occurs during install
/// after this constructor, and no lazy initialization can block another panic.
pub struct PrivateReportWriter {
    file: Option<Mutex<File>>,
}

impl PrivateReportWriter {
    /// Opens now; failures are represented by an unavailable writer. For error
    /// details, use [`Self::try_new`] during normal startup.
    #[must_use]
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self::try_new(path).unwrap_or(Self { file: None })
    }

    /// Opens the private file before installing the hook.
    ///
    /// # Errors
    /// Returns IO errors for unsafe paths, permissions or unavailable storage.
    pub fn try_new(path: impl AsRef<Path>) -> io::Result<Self> {
        open_private(path.as_ref()).map(|file| Self {
            file: Some(Mutex::new(file)),
        })
    }
}

impl ReportWriter for PrivateReportWriter {
    fn write_report(&self, report: &str) -> io::Result<()> {
        let file = self
            .file
            .as_ref()
            .ok_or_else(|| io::Error::other("private report unavailable"))?;
        let mut file = file
            .try_lock()
            .map_err(|_| io::Error::new(io::ErrorKind::WouldBlock, "private report busy"))?;
        file.write_all(report.as_bytes())
    }
}

/// Opens through directory descriptors and validates the resulting Linux inode.
#[cfg(target_os = "linux")]
fn open_private(path: &Path) -> io::Result<File> {
    use rustix::fs::{self, Mode, OFlags};
    use std::path::Component;
    let invalid = || {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe private report path or permissions",
        )
    };
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = fs::open(
        if path.is_absolute() { "/" } else { "." },
        flags,
        Mode::empty(),
    )?;
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        let name = match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => name,
            _ => return Err(invalid()),
        };
        if components.peek().is_some() {
            match fs::mkdirat(&directory, name, Mode::from_raw_mode(0o700)) {
                Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                Err(error) => return Err(error.into()),
            }
            directory = fs::openat(&directory, name, flags, Mode::empty())?;
        } else {
            let flags = OFlags::WRONLY
                | OFlags::APPEND
                | OFlags::NOFOLLOW
                | OFlags::CLOEXEC
                | OFlags::NONBLOCK;
            let file = match fs::openat(
                &directory,
                name,
                flags | OFlags::CREATE | OFlags::EXCL,
                Mode::from_raw_mode(0o600),
            ) {
                Ok(file) => file,
                Err(rustix::io::Errno::EXIST) => {
                    fs::openat(&directory, name, flags, Mode::empty())?
                }
                Err(error) => return Err(error.into()),
            };
            let stat = fs::fstat(&file)?;
            if fs::FileType::from_raw_mode(stat.st_mode) != fs::FileType::RegularFile
                || stat.st_uid != rustix::process::geteuid().as_raw()
                || stat.st_nlink != 1
                || stat.st_mode & 0o077 != 0
            {
                return Err(invalid());
            }
            return Ok(file.into());
        }
    }
    Err(invalid())
}

/// Rejects platforms whose private ACL semantics have not been implemented.
#[cfg(not(target_os = "linux"))]
fn open_private(_path: &Path) -> io::Result<File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "inject a writer enforcing private ACLs",
    ))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use aster_forge_test::temp::TestTempDir;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn private_append_and_busy_lock() {
        let dir = TestTempDir::new("panic-private");
        let path = dir.path().canonicalize().unwrap().join("nested/crash.log");
        let writer = PrivateReportWriter::try_new(&path).unwrap();
        writer.write_report("one\n").unwrap();
        PrivateReportWriter::try_new(&path)
            .unwrap()
            .write_report("two\n")
            .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\n");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let _guard = writer.file.as_ref().unwrap().lock().unwrap();
        assert_eq!(
            writer.write_report("blocked").unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn rejects_links_permissions_and_special_paths_without_mutation() {
        let dir = TestTempDir::new("panic-insecure");
        let root = dir.path().canonicalize().unwrap();
        let target = root.join("target");
        std::fs::write(&target, "original").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(PrivateReportWriter::try_new(&target).is_err());
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o644
        );
        symlink(&target, root.join("link")).unwrap();
        symlink(root.join("missing"), root.join("dangling")).unwrap();
        symlink(&root, root.join("directory-link")).unwrap();
        for path in [
            root.join("link"),
            root.join("dangling"),
            root.join("directory-link/crash"),
            target.join("crash"),
            root.join("../escape"),
        ] {
            assert!(PrivateReportWriter::try_new(path).is_err());
        }
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::hard_link(&target, root.join("hardlink")).unwrap();
        assert!(PrivateReportWriter::try_new(&target).is_err());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "original");
        assert!(PrivateReportWriter::try_new("/dev/null").is_err());
    }
}

#[cfg(all(test, target_os = "linux"))]
#[test]
fn unwritable_directory_fails_without_panic() {
    use std::os::unix::fs::PermissionsExt;
    if rustix::process::geteuid().is_root() {
        return;
    }
    let dir = aster_forge_test::temp::TestTempDir::new("panic-unwritable");
    let root = dir.path().canonicalize().unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();
    let writer = PrivateReportWriter::new(root.join("crash.log"));
    let result = writer.write_report("secret");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    assert!(!root.join("crash.log").exists());
}

/// Unsupported platforms must reject even safe-looking files before mutation.
#[cfg(all(test, not(target_os = "linux")))]
#[test]
fn unsupported_acl_platform_does_not_open_or_create() {
    let directory = aster_forge_test::temp::TestTempDir::new("panic-unsupported-acl");
    let existing = directory.join("existing");
    std::fs::write(&existing, "original").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    for path in [existing.clone(), directory.join("nested/new")] {
        let error = open_private(&path).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(
            PrivateReportWriter::new(&path)
                .write_report("secret")
                .is_err()
        );
    }
    assert_eq!(std::fs::read_to_string(existing).unwrap(), "original");
    assert!(!directory.join("nested").exists());
}
