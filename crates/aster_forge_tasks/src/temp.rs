//! Temporary directory helpers for background task artifacts.
//!
//! Task workers often write intermediate files under a token-scoped directory. These helpers keep
//! the shared directory layout and cleanup behavior in Forge while products keep ownership of when
//! a task is allowed to create or delete artifacts.

use crate::{Result, TaskCoreError, TaskLease};

/// Cleans a temporary directory tree, logging failures instead of returning them.
///
/// Missing directories are accepted. `DirectoryNotEmpty` is retried because some platforms and
/// filesystem watchers can briefly create files while a recursive removal is in progress.
pub async fn cleanup_temp_dir(path: &str) {
    aster_forge_utils::fs::cleanup_temp_dir(path).await;
}

/// Cleans the short-lived runtime temporary directory under `temp_root`.
pub async fn cleanup_runtime_temp_root(temp_root: &str) {
    aster_forge_utils::fs::cleanup_runtime_temp_root(temp_root).await;
}

/// Prepares the token-scoped temporary directory for one claimed task lease.
pub async fn prepare_task_temp_dir_in_root(temp_root: &str, lease: TaskLease) -> Result<String> {
    tracing::debug!(
        task_id = lease.task_id,
        processing_token = lease.processing_token,
        "preparing background task temp dir"
    );
    cleanup_task_temp_dir_for_lease_in_root(temp_root, lease).await?;
    let task_temp_dir = aster_forge_utils::paths::task_token_temp_dir(
        temp_root,
        lease.task_id,
        lease.processing_token,
    );
    tokio::fs::create_dir_all(&task_temp_dir)
        .await
        .map_err(|error| TaskCoreError::io(format!("create task temp dir: {error}")))?;
    tracing::debug!(
        task_id = lease.task_id,
        processing_token = lease.processing_token,
        "prepared background task temp dir"
    );
    Ok(task_temp_dir)
}

/// Cleans the token-scoped temporary directory for one claimed task lease.
pub async fn cleanup_task_temp_dir_for_lease_in_root(
    temp_root: &str,
    lease: TaskLease,
) -> Result<()> {
    tracing::debug!(
        task_id = lease.task_id,
        processing_token = lease.processing_token,
        "cleaning background task temp dir for lease"
    );
    cleanup_temp_dir(&aster_forge_utils::paths::task_token_temp_dir(
        temp_root,
        lease.task_id,
        lease.processing_token,
    ))
    .await;
    Ok(())
}

/// Cleans every temporary artifact directory for one persisted task id.
pub async fn cleanup_task_temp_dir_for_task_in_root(temp_root: &str, task_id: i64) -> Result<()> {
    tracing::debug!(task_id, "cleaning background task temp dir in root");
    cleanup_temp_dir(&aster_forge_utils::paths::task_temp_dir(temp_root, task_id)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::TaskLease;

    use super::{
        cleanup_runtime_temp_root, cleanup_task_temp_dir_for_lease_in_root,
        cleanup_task_temp_dir_for_task_in_root, cleanup_temp_dir, prepare_task_temp_dir_in_root,
    };

    static TEMP_ID: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "aster-forge-tasks-{label}-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[tokio::test]
    async fn cleanup_temp_dir_removes_directory_tree() {
        let path = unique_temp_path("cleanup");
        let nested = path.join("nested");
        tokio::fs::create_dir_all(&nested).await.unwrap();
        tokio::fs::write(nested.join("payload.txt"), b"temporary")
            .await
            .unwrap();

        cleanup_temp_dir(path.to_str().unwrap()).await;

        assert!(!path.exists());
    }

    #[tokio::test]
    async fn cleanup_temp_dir_tolerates_missing_directory() {
        let path = unique_temp_path("missing-cleanup");

        cleanup_temp_dir(path.to_str().unwrap()).await;

        assert!(!path.exists());
    }

    #[tokio::test]
    async fn prepare_task_temp_dir_creates_token_scoped_directory() {
        let root = unique_temp_path("prepare");
        let lease = TaskLease::new(42, 7);

        let prepared = prepare_task_temp_dir_in_root(root.to_str().unwrap(), lease)
            .await
            .expect("task temp dir should be prepared");

        assert!(PathBuf::from(&prepared).is_dir());
        cleanup_temp_dir(root.to_str().unwrap()).await;
    }

    #[tokio::test]
    async fn cleanup_task_temp_dir_for_lease_removes_only_token_dir() {
        let root = unique_temp_path("lease-cleanup");
        let lease = TaskLease::new(42, 7);
        let keep = aster_forge_utils::paths::task_token_temp_dir(root.to_str().unwrap(), 42, 8);
        let remove = prepare_task_temp_dir_in_root(root.to_str().unwrap(), lease)
            .await
            .expect("task temp dir should be prepared");
        tokio::fs::create_dir_all(&keep).await.unwrap();

        cleanup_task_temp_dir_for_lease_in_root(root.to_str().unwrap(), lease)
            .await
            .expect("lease cleanup should succeed");

        assert!(!PathBuf::from(remove).exists());
        assert!(PathBuf::from(&keep).is_dir());
        cleanup_temp_dir(root.to_str().unwrap()).await;
    }

    #[tokio::test]
    async fn cleanup_task_temp_dir_for_task_removes_all_token_dirs() {
        let root = unique_temp_path("task-cleanup");
        let lease = TaskLease::new(42, 7);
        prepare_task_temp_dir_in_root(root.to_str().unwrap(), lease)
            .await
            .expect("task temp dir should be prepared");

        cleanup_task_temp_dir_for_task_in_root(root.to_str().unwrap(), 42)
            .await
            .expect("task cleanup should succeed");

        assert!(
            !PathBuf::from(aster_forge_utils::paths::task_temp_dir(
                root.to_str().unwrap(),
                42
            ))
            .exists()
        );
        cleanup_temp_dir(root.to_str().unwrap()).await;
    }

    #[tokio::test]
    async fn cleanup_runtime_temp_root_removes_runtime_namespace_only() {
        let root = unique_temp_path("runtime-cleanup");
        let runtime = aster_forge_utils::paths::runtime_temp_dir(root.to_str().unwrap());
        let keep = root.join("tasks");
        tokio::fs::create_dir_all(&runtime).await.unwrap();
        tokio::fs::create_dir_all(&keep).await.unwrap();

        cleanup_runtime_temp_root(root.to_str().unwrap()).await;

        assert!(!PathBuf::from(runtime).exists());
        assert!(keep.is_dir());
        cleanup_temp_dir(root.to_str().unwrap()).await;
    }
}
