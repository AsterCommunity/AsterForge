//! Best-effort filesystem cleanup helpers.
//!
//! These helpers are for temporary artifacts where cleanup failures should be logged without
//! masking the primary operation result. They intentionally do not return errors. Callers that need
//! transactional deletion, user-visible failures, or storage-driver semantics should keep using
//! explicit filesystem or storage APIs at the product boundary.

use std::io::ErrorKind;
use std::path::Path;
use std::time::Duration;

const CLEANUP_RETRY_ATTEMPTS: usize = 3;
const CLEANUP_RETRY_DELAY: Duration = Duration::from_millis(50);

/// Removes a temporary file, ignoring missing files and logging other failures.
pub async fn cleanup_temp_file(path: impl AsRef<Path>) {
    let path = path.as_ref();
    if let Err(error) = tokio::fs::remove_file(path).await
        && error.kind() != ErrorKind::NotFound
    {
        tracing::warn!(
            path = %path.display(),
            error = %error,
            "failed to cleanup temp file"
        );
    }
}

/// Removes a temporary directory tree, ignoring missing directories and logging other failures.
///
/// `DirectoryNotEmpty` is retried because some platforms and filesystem watchers can briefly
/// create files while a recursive removal is in progress.
pub async fn cleanup_temp_dir(path: impl AsRef<Path>) {
    let path = path.as_ref();
    for _ in 0..CLEANUP_RETRY_ATTEMPTS {
        match tokio::fs::remove_dir_all(path).await {
            Ok(()) => return,
            Err(error) if error.kind() == ErrorKind::NotFound => return,
            Err(error) if error.kind() == ErrorKind::DirectoryNotEmpty => {
                tokio::time::sleep(CLEANUP_RETRY_DELAY).await;
            }
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %error,
                    "failed to cleanup temp dir"
                );
                return;
            }
        }
    }

    if let Err(error) = tokio::fs::remove_dir_all(path).await
        && error.kind() != ErrorKind::NotFound
    {
        tracing::warn!(
            path = %path.display(),
            error = %error,
            "failed to cleanup temp dir"
        );
    }
}

/// Removes the short-lived runtime temporary directory under `temp_root`.
pub async fn cleanup_runtime_temp_root(temp_root: &str) {
    cleanup_temp_dir(crate::paths::runtime_temp_dir(temp_root)).await;
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{cleanup_runtime_temp_root, cleanup_temp_dir, cleanup_temp_file};

    static TEMP_ID: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "aster-forge-utils-{label}-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[tokio::test]
    async fn cleanup_temp_file_removes_file() {
        let path = unique_temp_path("file-cleanup");
        tokio::fs::write(&path, b"temporary")
            .await
            .expect("temp file should be created");

        cleanup_temp_file(&path).await;

        assert!(!path.exists());
    }

    #[tokio::test]
    async fn cleanup_temp_file_tolerates_missing_file() {
        let path = unique_temp_path("missing-file-cleanup");

        cleanup_temp_file(&path).await;

        assert!(!path.exists());
    }

    #[tokio::test]
    async fn cleanup_temp_dir_removes_directory_tree() {
        let path = unique_temp_path("dir-cleanup");
        let nested = path.join("nested");
        tokio::fs::create_dir_all(&nested)
            .await
            .expect("nested temp dir should be created");
        tokio::fs::write(nested.join("payload.txt"), b"temporary")
            .await
            .expect("nested temp file should be created");

        cleanup_temp_dir(&path).await;

        assert!(!path.exists());
    }

    #[tokio::test]
    async fn cleanup_temp_dir_tolerates_missing_directory() {
        let path = unique_temp_path("missing-dir-cleanup");

        cleanup_temp_dir(&path).await;

        assert!(!path.exists());
    }

    #[tokio::test]
    async fn cleanup_runtime_temp_root_removes_runtime_namespace_only() {
        let root = unique_temp_path("runtime-cleanup");
        let runtime = crate::paths::runtime_temp_dir(root.to_str().expect("path should be utf-8"));
        let keep = root.join("tasks");
        tokio::fs::create_dir_all(&runtime)
            .await
            .expect("runtime temp dir should be created");
        tokio::fs::create_dir_all(&keep)
            .await
            .expect("task temp dir should be created");

        cleanup_runtime_temp_root(root.to_str().expect("path should be utf-8")).await;

        assert!(!PathBuf::from(runtime).exists());
        assert!(keep.is_dir());
        cleanup_temp_dir(root).await;
    }
}
