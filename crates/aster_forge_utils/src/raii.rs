//! RAII cleanup guards for short-lived runtime resources.
//!
//! The guards remove temporary files or directories when they leave scope, covering early returns
//! and panic unwinding. They are deliberately small and best-effort; startup cleanup should still
//! handle resources left behind by process termination.

use std::path::{Path, PathBuf};

/// RAII guard for short-lived runtime temporary files.
///
/// It prevents cleanup from being skipped on early returns or panic unwinding. Files left behind
/// after process termination should still be handled by startup runtime-temp cleanup.
pub struct TempFileGuard {
    path: PathBuf,
    cleanup_label: &'static str,
}

impl TempFileGuard {
    /// Creates a guard that removes `path` on drop.
    pub fn new(path: PathBuf, cleanup_label: &'static str) -> Self {
        Self {
            path,
            cleanup_label,
        }
    }

    /// Returns the guarded path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                path = ?self.path,
                cleanup = self.cleanup_label,
                "failed to cleanup temp file: {error}"
            );
        }
    }
}

/// RAII guard for short-lived runtime temporary directories.
///
/// Directories left behind after process termination should still be handled by startup
/// runtime-temp cleanup.
pub struct TempDirGuard {
    path: PathBuf,
    cleanup_label: &'static str,
}

impl TempDirGuard {
    /// Creates a guard that removes `path` recursively on drop.
    pub fn new(path: PathBuf, cleanup_label: &'static str) -> Self {
        Self {
            path,
            cleanup_label,
        }
    }

    /// Returns the guarded path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                path = %self.path.display(),
                cleanup = self.cleanup_label,
                "failed to cleanup temp dir: {error}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TempDirGuard, TempFileGuard};
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("aster-forge-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn temp_file_guard_removes_file_on_drop() {
        let path = temp_path("file-guard");
        std::fs::write(&path, b"temporary").expect("temp file should be created");

        {
            let guard = TempFileGuard::new(path.clone(), "test-temp-file");
            assert_eq!(guard.path(), path.as_path());
            assert!(path.exists());
        }

        assert!(!path.exists());
    }

    #[test]
    fn temp_file_guard_ignores_missing_file() {
        let path = temp_path("missing-file-guard");
        {
            let guard = TempFileGuard::new(path.clone(), "test-missing-temp-file");
            assert_eq!(guard.path(), path.as_path());
        }

        assert!(!path.exists());
    }

    #[test]
    fn temp_dir_guard_removes_directory_tree_on_drop() {
        let path = temp_path("dir-guard");
        let nested = path.join("nested");
        std::fs::create_dir_all(&nested).expect("nested temp dir should be created");
        std::fs::write(nested.join("file.txt"), b"temporary")
            .expect("nested temp file should be created");

        {
            let guard = TempDirGuard::new(path.clone(), "test-temp-dir");
            assert_eq!(guard.path(), path.as_path());
            assert!(nested.exists());
        }

        assert!(!path.exists());
    }

    #[test]
    fn temp_dir_guard_ignores_missing_directory() {
        let path = temp_path("missing-dir-guard");
        {
            let guard = TempDirGuard::new(path.clone(), "test-missing-temp-dir");
            assert_eq!(guard.path(), path.as_path());
        }

        assert!(!path.exists());
    }
}
