//! Unix state files anchored to a verified, owner-controlled directory descriptor.
//!
//! Shared by process coordination and durable adapter fixtures. This owns filesystem mechanics,
//! never the caller's path selection, document schema, locking protocol, or recovery policy.

use std::fs::{DirBuilder, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Component, Path};

use rustix::fs::{AtFlags, FileType, Mode, OFlags, fstat, open, openat, renameat, unlinkat};

/// An open directory owned by the effective user and not writable by other users.
#[derive(Debug)]
pub struct StateDirectory {
    directory: File,
}

impl StateDirectory {
    /// Creates an owner-only directory or verifies an existing owner-controlled directory.
    ///
    /// # Errors
    /// Rejects symlinks, foreign ownership, group/world write permission, and filesystem failures.
    /// The parent must exist; this never changes permissions on an existing directory.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        match DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let directory = File::from(open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let stat = fstat(&directory)?;
        if stat.st_uid != rustix::process::geteuid().as_raw() || stat.st_mode & 0o022 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "state directory must be owned by the current user and not writable by others",
            ));
        }
        Ok(Self { directory })
    }

    /// Opens a regular owner-controlled lock file without truncation or following symlinks.
    ///
    /// # Errors
    /// Rejects non-single-component names, symlinks, foreign or hard-linked files and I/O failures.
    pub fn open_lock(&self, name: &str) -> io::Result<File> {
        validate_name(name)?;
        let file = File::from(openat(
            &self.directory,
            name,
            OFlags::CREATE | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        verify_file(&file)?;
        Ok(file)
    }

    /// Reads an existing regular state file without following symlinks.
    ///
    /// # Errors
    /// Returns `NotFound` for a missing file, or an error for an unsafe name/file or failed read.
    pub fn read(&self, name: &str, maximum_bytes: usize) -> io::Result<Vec<u8>> {
        if maximum_bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "state read limit must be positive",
            ));
        }
        validate_name(name)?;
        let file = File::from(openat(
            &self.directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        verify_file(&file)?;
        let mut bytes = Vec::new();
        let limit = u64::try_from(maximum_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        file.take(limit).read_to_end(&mut bytes)?;
        if bytes.len() > maximum_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "state document exceeds the byte limit",
            ));
        }
        Ok(bytes)
    }

    /// Atomically replaces a document using a fresh owner-only file and syncs file and directory.
    ///
    /// # Errors
    /// Returns errors for unsafe names or create/write/sync/rename failures. Failed temporary
    /// writes are removed; the previous document remains intact until the successful rename.
    pub fn atomic_write(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        validate_name(name)?;
        let temporary = format!(".{name}.{}.tmp", uuid::Uuid::new_v4());
        let result = (|| {
            let mut file = File::from(openat(
                &self.directory,
                temporary.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )?);
            file.write_all(bytes)?;
            file.sync_all()?;
            renameat(&self.directory, temporary.as_str(), &self.directory, name)?;
            self.directory.sync_all()
        })();
        if result.is_err() {
            let _ = unlinkat(&self.directory, temporary.as_str(), AtFlags::empty());
        }
        result
    }
}

fn verify_file(file: &File) -> io::Result<()> {
    let stat = fstat(file)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || stat.st_nlink != 1
        || stat.st_mode & 0o022 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "state file must be regular, single-linked, owned by the current user and not writable by others",
        ));
    }
    Ok(())
}

fn validate_name(name: &str) -> io::Result<()> {
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
        || name.contains(['/', '\\', '\0'])
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "state file name must be a single normal component",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    fn fixture() -> (std::path::PathBuf, crate::raii::TempDirGuard) {
        let path = std::env::temp_dir().join(format!("forge-state-{}", uuid::Uuid::new_v4()));
        (
            path.clone(),
            crate::raii::TempDirGuard::new(path, "state security test"),
        )
    }

    #[test]
    fn lock_open_never_truncates_or_follows_links() {
        let (path, _guard) = fixture();
        let directory = StateDirectory::open(&path).unwrap();
        std::fs::write(path.join("victim"), b"keep").unwrap();
        symlink(path.join("victim"), path.join("lock")).unwrap();
        assert!(directory.open_lock("lock").is_err());
        assert_eq!(std::fs::read(path.join("victim")).unwrap(), b"keep");
        let mut lock = directory.open_lock("real-lock").unwrap();
        lock.write_all(b"keep-lock").unwrap();
        directory.open_lock("real-lock").unwrap();
        assert_eq!(directory.read("real-lock", 1024).unwrap(), b"keep-lock");
        std::fs::hard_link(path.join("victim"), path.join("hard-lock")).unwrap();
        assert!(directory.open_lock("hard-lock").is_err());
    }

    #[test]
    fn atomic_state_is_private_and_does_not_follow_old_temporary_or_destination_links() {
        let (path, _guard) = fixture();
        let directory = StateDirectory::open(&path).unwrap();
        std::fs::write(path.join("victim"), b"keep").unwrap();
        symlink(path.join("victim"), path.join("writeback.json.tmp")).unwrap();
        symlink(path.join("victim"), path.join("writeback.json")).unwrap();
        assert!(directory.read("writeback.json", 1024).is_err());
        directory
            .atomic_write("writeback.json", b"private")
            .unwrap();
        assert_eq!(directory.read("writeback.json", 1024).unwrap(), b"private");
        assert_eq!(std::fs::read(path.join("victim")).unwrap(), b"keep");
        assert_eq!(
            std::fs::metadata(path.join("writeback.json"))
                .unwrap()
                .mode()
                & 0o777,
            0o600
        );
        for name in ["", ".", "..", "../escape", "/absolute", "a/b", "a\\b"] {
            assert!(directory.atomic_write(name, b"x").is_err(), "{name}");
        }
    }

    #[test]
    fn refuses_unsafe_directory_permissions_and_directory_symlinks() {
        let (path, _guard) = fixture();
        StateDirectory::open(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(StateDirectory::open(&path).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = path.join("alias");
        symlink(&path, &link).unwrap();
        assert!(StateDirectory::open(&link).is_err());
    }
    #[test]
    fn state_read_enforces_exact_byte_limit_and_rejects_non_regular_files() {
        let (path, _guard) = fixture();
        let directory = StateDirectory::open(&path).unwrap();
        directory.atomic_write("data", b"1234").unwrap();
        assert_eq!(directory.read("data", 4).unwrap(), b"1234");
        assert!(directory.read("data", 3).is_err());
        assert!(directory.read("data", 0).is_err());
        std::fs::create_dir(path.join("non-regular")).unwrap();
        assert!(directory.open_lock("non-regular").is_err());
        assert!(directory.read("non-regular", 4).is_err());
    }
}
