use chrono::Utc;

use crate::db::repository::background_task_repo;
use crate::errors::{AsterError, Result};
use crate::runtime::{AppState, SharedRuntimeState};
use crate::types::BackgroundTaskStatus;

use super::{DispatchStats, TASK_DRAIN_MAX_ROUNDS, dispatch_due};

pub async fn drain(state: &AppState) -> Result<DispatchStats> {
    let mut total = DispatchStats::default();

    for _ in 0..TASK_DRAIN_MAX_ROUNDS {
        let stats = dispatch_due(state).await?;
        let claimed = stats.claimed;
        total.claimed += stats.claimed;
        total.succeeded += stats.succeeded;
        total.retried += stats.retried;
        total.failed += stats.failed;
        if claimed > 0 {
            continue;
        }

        if background_task_repo::count_processing(state.writer_db()).await? == 0 {
            break;
        }

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    Ok(total)
}

pub async fn cleanup_expired(state: &impl SharedRuntimeState) -> Result<u64> {
    cleanup_expired_in_root(state, &state.config().server.temp_dir).await
}

async fn cleanup_expired_in_root(state: &impl SharedRuntimeState, temp_root: &str) -> Result<u64> {
    let now = Utc::now();
    let tasks_root = crate::utils::paths::temp_file_path(temp_root, "tasks");
    let mut entries = match tokio::fs::read_dir(&tasks_root).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(AsterError::internal_error(format!(
                "read task temp root {tasks_root}: {error}"
            )));
        }
    };
    let mut cleaned = 0;

    while let Some(entry) = entries.next_entry().await.map_err(|error| {
        AsterError::internal_error(format!("iterate task temp root {tasks_root}: {error}"))
    })? {
        let path = entry.path();
        let path_display = path.to_string_lossy().to_string();
        let file_type = entry.file_type().await.map_err(|error| {
            AsterError::internal_error(format!("read task temp entry type {path_display}: {error}"))
        })?;
        if !file_type.is_dir() {
            continue;
        }

        let dir_name = entry.file_name();
        let Some(dir_name) = dir_name.to_str() else {
            tracing::warn!(path = %path_display, "skipping task temp dir with non-utf8 name");
            continue;
        };
        let Ok(task_id) = dir_name.parse::<i64>() else {
            tracing::warn!(path = %path_display, "skipping task temp dir with invalid task id");
            continue;
        };

        // 这里只删“产物目录”，不删 background_task 记录：
        // - 终态且 expires_at 已到的任务：删 temp 目录，保留历史行；
        // - 数据库里已经没有任务行的孤儿目录：直接删，避免长期泄露磁盘。
        let should_cleanup =
            match background_task_repo::find_by_id(state.writer_db(), task_id).await {
                Ok(task) => {
                    task.expires_at <= now
                        && matches!(
                            task.status,
                            BackgroundTaskStatus::Succeeded
                                | BackgroundTaskStatus::Failed
                                | BackgroundTaskStatus::Canceled
                        )
                }
                Err(AsterError::RecordNotFound(_)) => {
                    tracing::warn!(
                        task_id,
                        path = %path_display,
                        "cleaning orphaned task temp dir without task record"
                    );
                    true
                }
                Err(error) => return Err(error),
            };
        if !should_cleanup {
            continue;
        }

        crate::utils::cleanup_temp_dir(&path_display).await;
        let still_exists = tokio::fs::try_exists(&path).await.map_err(|error| {
            AsterError::internal_error(format!(
                "verify task temp dir cleanup {path_display}: {error}"
            ))
        })?;
        if still_exists {
            tracing::warn!(
                task_id,
                path = %path_display,
                "task temp dir still exists after cleanup attempt"
            );
            continue;
        }

        cleaned += 1;
    }

    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use super::cleanup_expired_in_root;
    use crate::entities::background_task;
    use crate::runtime::SharedRuntimeState;
    use crate::types::{BackgroundTaskKind, BackgroundTaskStatus, StoredTaskPayload};
    use chrono::{Duration, Utc};
    use sea_orm::{ActiveModelTrait, Set};
    use std::sync::Arc;

    async fn test_state(temp_dir: String) -> crate::runtime::AppState {
        let db_cfg = crate::config::DatabaseConfig {
            url: "sqlite::memory:".to_string(),
            pool_size: 1,
            retry_count: 0,
        };
        let db = crate::db::connect_with_metrics(&db_cfg, crate::metrics_core::NoopMetrics::arc())
            .await
            .expect("maintenance test database should connect");
        migration::Migrator::up(&db, None)
            .await
            .expect("maintenance test migrations should run");
        crate::services::system_config_service::ensure_defaults(&db)
            .await
            .expect("maintenance test defaults should seed");

        let runtime_config = Arc::new(crate::config::RuntimeConfig::new());
        runtime_config
            .reload(&db)
            .await
            .expect("maintenance runtime config should reload");
        let config = Arc::new(crate::config::Config {
            server: crate::config::ServerConfig {
                temp_dir,
                ..Default::default()
            },
            database: db_cfg,
            cache: crate::config::CacheConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        });
        let cache = crate::cache::create_cache(&config.cache).await;

        crate::runtime::AppState {
            db_handles: crate::db::DbHandles::single(db),
            config,
            runtime_config,
            cache,
            metrics: crate::metrics_core::NoopMetrics::arc(),
            background_task_dispatch_wakeup:
                crate::runtime::AppState::new_background_task_dispatch_wakeup(),
        }
    }

    async fn insert_task(
        state: &crate::runtime::AppState,
        status: BackgroundTaskStatus,
        expires_at: chrono::DateTime<Utc>,
    ) -> i64 {
        let now = Utc::now();
        let task = background_task::ActiveModel {
            kind: Set(BackgroundTaskKind::SystemRuntime),
            status: Set(status),
            creator_user_id: Set(None),
            display_name: Set("cleanup candidate".to_string()),
            payload_json: Set(StoredTaskPayload(
                serde_json::json!({ "task_name": "task-cleanup" }).to_string(),
            )),
            result_json: Set(None),
            runtime_json: Set(None),
            steps_json: Set(None),
            progress_current: Set(0),
            progress_total: Set(1),
            status_text: Set(None),
            attempt_count: Set(0),
            max_attempts: Set(1),
            next_run_at: Set(now),
            processing_token: Set(0),
            processing_started_at: Set(None),
            last_heartbeat_at: Set(None),
            lease_expires_at: Set(None),
            started_at: Set(Some(now - Duration::minutes(1))),
            finished_at: Set(status.is_terminal().then_some(now)),
            last_error: Set(None),
            failure_can_retry: Set(None),
            expires_at: Set(expires_at),
            created_at: Set(now - Duration::hours(1)),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(state.writer_db())
        .await
        .expect("maintenance test task should insert");
        task.id
    }

    #[tokio::test]
    async fn cleanup_expired_removes_only_expired_terminal_and_orphan_task_dirs() {
        let temp_root = format!("/tmp/asterforge-maintenance-{}", uuid::Uuid::new_v4());
        let state = test_state(temp_root.clone()).await;
        let now = Utc::now();
        let expired_terminal = insert_task(
            &state,
            BackgroundTaskStatus::Succeeded,
            now - Duration::seconds(1),
        )
        .await;
        let unexpired_terminal = insert_task(
            &state,
            BackgroundTaskStatus::Failed,
            now + Duration::hours(1),
        )
        .await;
        let active_expired = insert_task(
            &state,
            BackgroundTaskStatus::Processing,
            now - Duration::hours(1),
        )
        .await;
        let orphan_id = expired_terminal + unexpired_terminal + active_expired + 1000;

        for task_id in [
            expired_terminal,
            unexpired_terminal,
            active_expired,
            orphan_id,
        ] {
            tokio::fs::create_dir_all(crate::utils::paths::task_temp_dir(&temp_root, task_id))
                .await
                .unwrap();
        }
        let tasks_root = crate::utils::paths::temp_file_path(&temp_root, "tasks");
        tokio::fs::write(format!("{tasks_root}/not-a-dir"), b"ignored")
            .await
            .unwrap();
        tokio::fs::create_dir_all(format!("{tasks_root}/not-an-id"))
            .await
            .unwrap();

        let cleaned = cleanup_expired_in_root(&state, &temp_root).await.unwrap();
        assert_eq!(cleaned, 2);
        assert!(
            !tokio::fs::try_exists(crate::utils::paths::task_temp_dir(
                &temp_root,
                expired_terminal
            ))
            .await
            .unwrap()
        );
        assert!(
            tokio::fs::try_exists(crate::utils::paths::task_temp_dir(
                &temp_root,
                unexpired_terminal
            ))
            .await
            .unwrap()
        );
        assert!(
            tokio::fs::try_exists(crate::utils::paths::task_temp_dir(
                &temp_root,
                active_expired
            ))
            .await
            .unwrap()
        );
        assert!(
            !tokio::fs::try_exists(crate::utils::paths::task_temp_dir(&temp_root, orphan_id))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn cleanup_expired_returns_zero_when_task_temp_root_is_missing() {
        let temp_root = format!(
            "/tmp/asterforge-maintenance-missing-{}",
            uuid::Uuid::new_v4()
        );
        let state = test_state(temp_root.clone()).await;

        assert_eq!(
            cleanup_expired_in_root(&state, &temp_root).await.unwrap(),
            0
        );
    }
}
