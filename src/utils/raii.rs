//! RAII cleanup guards for short-lived runtime resources.

use std::path::{Path, PathBuf};

/// RAII 临时文件守卫。用于短生命周期 runtime 临时文件，防止错误返回或 panic unwind
/// 时遗漏清理；进程被 kill 后的残留仍由启动时 runtime temp 清理兜底。
pub struct TempFileGuard {
    path: PathBuf,
    cleanup_label: &'static str,
}

impl TempFileGuard {
    pub fn new(path: PathBuf, cleanup_label: &'static str) -> Self {
        Self {
            path,
            cleanup_label,
        }
    }

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

/// RAII 临时目录守卫。用于短生命周期 runtime 临时目录；如果进程异常退出，
/// 下次启动的 runtime temp 清理仍会兜底处理残留目录。
pub struct TempDirGuard {
    path: PathBuf,
    cleanup_label: &'static str,
}

impl TempDirGuard {
    pub fn new(path: PathBuf, cleanup_label: &'static str) -> Self {
        Self {
            path,
            cleanup_label,
        }
    }

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
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_ID: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "asterforge-raii-{label}-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn temp_file_guard_removes_file_on_drop_and_exposes_path() {
        let path = unique_temp_path("file");
        std::fs::write(&path, b"temporary").unwrap();

        {
            let guard = TempFileGuard::new(path.clone(), "test-temp-file");
            assert_eq!(guard.path(), path.as_path());
            assert!(path.exists());
        }

        assert!(!path.exists());
    }

    #[test]
    fn temp_file_guard_tolerates_missing_file() {
        let path = unique_temp_path("missing-file");
        let guard = TempFileGuard::new(path.clone(), "test-missing-file");

        drop(guard);

        assert!(!path.exists());
    }

    #[test]
    fn temp_dir_guard_removes_directory_tree_on_drop_and_exposes_path() {
        let path = unique_temp_path("dir");
        let nested = path.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("payload.txt"), b"temporary").unwrap();

        {
            let guard = TempDirGuard::new(path.clone(), "test-temp-dir");
            assert_eq!(guard.path(), path.as_path());
            assert!(nested.join("payload.txt").exists());
        }

        assert!(!path.exists());
    }

    #[test]
    fn temp_dir_guard_tolerates_missing_directory() {
        let path = unique_temp_path("missing-dir");
        let guard = TempDirGuard::new(path.clone(), "test-missing-dir");

        drop(guard);

        assert!(!path.exists());
    }
}
