//! Buffered audit-log persistence backed by [`aster_forge_db`].

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use aster_forge_db::{AuditLogCreate, create_audit_log_requests, create_audit_log_row};
use sea_orm::DatabaseConnection;

/// Default number of audit records retained in memory before direct-write fallback.
pub const DEFAULT_AUDIT_LOG_QUEUE_CAPACITY: usize = 4096;
/// Default number of audit records that triggers an immediate batch flush.
pub const DEFAULT_AUDIT_LOG_BATCH_SIZE: usize = 100;
/// Default delay before a partial audit batch is flushed.
pub const DEFAULT_AUDIT_LOG_DELAYED_FLUSH_AFTER: Duration = Duration::from_secs(1);

/// Buffering policy for [`AuditLogManager`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditLogBufferConfig {
    /// Maximum records retained in memory before overflow records are written directly.
    pub queue_capacity: usize,
    /// Number of queued records that triggers an immediate batch flush.
    pub batch_size: usize,
    /// Delay before a partial batch is flushed.
    pub delayed_flush_after: Duration,
}

impl AuditLogBufferConfig {
    /// Creates an audit-log buffering policy.
    pub const fn new(
        queue_capacity: usize,
        batch_size: usize,
        delayed_flush_after: Duration,
    ) -> Self {
        Self {
            queue_capacity,
            batch_size,
            delayed_flush_after,
        }
    }
}

impl Default for AuditLogBufferConfig {
    fn default() -> Self {
        Self::new(
            DEFAULT_AUDIT_LOG_QUEUE_CAPACITY,
            DEFAULT_AUDIT_LOG_BATCH_SIZE,
            DEFAULT_AUDIT_LOG_DELAYED_FLUSH_AFTER,
        )
    }
}

static GLOBAL_AUDIT_LOG_MANAGER: OnceLock<Arc<AuditLogManager>> = OnceLock::new();

/// Best-effort buffered writer for Forge audit-log rows.
pub struct AuditLogManager {
    writer: Arc<aster_forge_runtime::BufferedBatchWriter<AuditLogCreate>>,
}

impl AuditLogManager {
    /// Creates a manager with the default buffering policy.
    pub fn new(db: DatabaseConnection) -> Self {
        Self::with_config(db, AuditLogBufferConfig::default())
    }

    /// Creates a manager with a caller-provided buffering policy.
    pub fn with_config(db: DatabaseConnection, config: AuditLogBufferConfig) -> Self {
        let batch_size = config.batch_size.max(1);
        let batch_db = db.clone();
        let single_db = db;
        let writer = aster_forge_runtime::BufferedBatchWriter::new(
            aster_forge_runtime::BufferedBatchConfig::new(
                config.queue_capacity.max(1),
                batch_size,
                config.delayed_flush_after,
                "audit_log",
            ),
            move |batch| {
                let db = batch_db.clone();
                async move { write_audit_batch(&db, batch, batch_size).await }
            },
            move |request| {
                let db = single_db.clone();
                async move { write_audit_log_direct(&db, request).await }
            },
        );
        Self {
            writer: Arc::new(writer),
        }
    }

    /// Queues an audit record, falling back to a direct write if the queue is full.
    pub async fn record(&self, request: AuditLogCreate) {
        self.writer.record(request).await;
    }

    /// Flushes all currently buffered audit records.
    pub async fn flush(&self) {
        self.writer.flush().await;
    }

    /// Cancels delayed flush tasks. Call [`AuditLogManager::flush`] afterwards on shutdown.
    pub fn cancel(&self) {
        self.writer.cancel();
    }
}

/// Initializes the process-global audit manager.
///
/// Returns `true` when this call installed the manager and `false` when a manager was already
/// initialized.
pub fn init_global_audit_log_manager(db: DatabaseConnection) -> bool {
    let installed = GLOBAL_AUDIT_LOG_MANAGER
        .set(Arc::new(AuditLogManager::new(db)))
        .is_ok();
    if !installed {
        tracing::warn!("global audit log manager is already initialized; ignoring");
    }
    installed
}

/// Returns the process-global audit manager when initialized.
pub fn global_audit_log_manager() -> Option<&'static Arc<AuditLogManager>> {
    GLOBAL_AUDIT_LOG_MANAGER.get()
}

/// Records through the global manager, or writes directly through `fallback_db` before startup.
pub async fn record_audit_log(fallback_db: &DatabaseConnection, request: AuditLogCreate) {
    if let Some(manager) = global_audit_log_manager() {
        manager.record(request).await;
    } else {
        write_audit_log_direct(fallback_db, request).await;
    }
}

/// Flushes the process-global audit manager when initialized.
pub async fn flush_global_audit_log_manager() {
    if let Some(manager) = global_audit_log_manager() {
        manager.flush().await;
    }
}

/// Cancels delayed writes and flushes the process-global audit manager when initialized.
pub async fn shutdown_global_audit_log_manager() {
    if let Some(manager) = global_audit_log_manager() {
        manager.cancel();
        manager.flush().await;
    }
}

/// Writes one audit record directly, logging persistence failures as best-effort warnings.
pub async fn write_audit_log_direct(db: &DatabaseConnection, request: AuditLogCreate) {
    if let Err(error) = create_audit_log_row(db, request).await {
        tracing::warn!(%error, "failed to write audit log");
    }
}

async fn write_audit_batch(db: &DatabaseConnection, batch: Vec<AuditLogCreate>, batch_size: usize) {
    let total = batch.len();
    let mut requests = batch.into_iter();
    loop {
        let chunk = requests.by_ref().take(batch_size).collect::<Vec<_>>();
        if chunk.is_empty() {
            break;
        }

        let count = chunk.len();
        if let Err(error) = create_audit_log_requests(db, chunk).await {
            tracing::warn!(count, total, %error, "failed to write audit log batch");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use aster_forge_db::{AuditLogCreate, audit_log};
    use chrono::Utc;
    use sea_orm::{ConnectionTrait, DatabaseConnection, EntityTrait, PaginatorTrait};

    use super::{
        AuditLogBufferConfig, AuditLogManager, DEFAULT_AUDIT_LOG_BATCH_SIZE,
        DEFAULT_AUDIT_LOG_QUEUE_CAPACITY,
    };

    async fn test_db() -> DatabaseConnection {
        let db = sea_orm::Database::connect("sqlite::memory:")
            .await
            .expect("audit writer test database should connect");
        let backend = db.get_database_backend();
        let create_table = aster_forge_db::create_audit_logs_table(backend);
        db.execute(&create_table)
            .await
            .expect("audit writer test table should be created");
        db
    }

    fn audit_request(index: i64) -> AuditLogCreate {
        AuditLogCreate {
            user_id: 42,
            action: "file_upload".to_string(),
            entity_type: "file".to_string(),
            entity_id: Some(index),
            entity_name: Some(format!("file-{index}.txt")),
            details: None,
            ip_address: Some("127.0.0.1".to_string()),
            user_agent: Some("audit-writer-test".to_string()),
            created_at: Utc::now(),
        }
    }

    async fn audit_log_count(db: &DatabaseConnection) -> u64 {
        audit_log::Entity::find()
            .count(db)
            .await
            .expect("audit writer count query should succeed")
    }

    async fn wait_for_audit_log_count(db: &DatabaseConnection, expected: u64) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let current = audit_log_count(db).await;
            if current == expected {
                return;
            }
            assert!(
                current < expected,
                "audit count exceeded {expected}: {current}"
            );
            assert!(
                Instant::now() < deadline,
                "timed out waiting for audit count {expected}; last count was {current}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn test_manager(db: DatabaseConnection, delayed_flush_after: Duration) -> AuditLogManager {
        AuditLogManager::with_config(
            db,
            AuditLogBufferConfig::new(
                DEFAULT_AUDIT_LOG_QUEUE_CAPACITY,
                DEFAULT_AUDIT_LOG_BATCH_SIZE,
                delayed_flush_after,
            ),
        )
    }

    #[tokio::test]
    async fn flushes_threshold_batch() {
        let db = test_db().await;
        let manager = test_manager(db.clone(), Duration::from_secs(5));

        for index in 0..DEFAULT_AUDIT_LOG_BATCH_SIZE {
            manager.record(audit_request(index as i64)).await;
        }

        wait_for_audit_log_count(&db, DEFAULT_AUDIT_LOG_BATCH_SIZE as u64).await;
        manager.cancel();
    }

    #[tokio::test]
    async fn flushes_partial_batch_after_delay() {
        let db = test_db().await;
        let manager = test_manager(db.clone(), Duration::from_millis(20));

        for index in 0..3 {
            manager.record(audit_request(index)).await;
        }

        wait_for_audit_log_count(&db, 3).await;
        manager.cancel();
    }

    #[tokio::test]
    async fn partial_batch_waits_for_configured_delay() {
        let db = test_db().await;
        let manager = test_manager(db.clone(), Duration::from_millis(120));

        manager.record(audit_request(1)).await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(audit_log_count(&db).await, 0);

        wait_for_audit_log_count(&db, 1).await;
        manager.cancel();
    }

    #[tokio::test]
    async fn cancelled_shutdown_flushes_buffer() {
        let db = test_db().await;
        let manager = test_manager(db.clone(), Duration::from_secs(5));

        manager.record(audit_request(1)).await;
        manager.cancel();
        manager.flush().await;

        assert_eq!(audit_log_count(&db).await, 1);
    }

    #[tokio::test]
    async fn manual_flush_allows_later_delayed_flush() {
        let db = test_db().await;
        let manager = test_manager(db.clone(), Duration::from_millis(20));

        manager.record(audit_request(1)).await;
        manager.flush().await;
        assert_eq!(audit_log_count(&db).await, 1);

        manager.record(audit_request(2)).await;
        wait_for_audit_log_count(&db, 2).await;
        manager.cancel();
    }

    #[tokio::test]
    async fn cancel_stops_delayed_flush_until_explicit_flush() {
        let db = test_db().await;
        let manager = test_manager(db.clone(), Duration::from_millis(20));

        manager.record(audit_request(1)).await;
        manager.cancel();
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(audit_log_count(&db).await, 0);

        manager.flush().await;
        assert_eq!(audit_log_count(&db).await, 1);
    }

    #[tokio::test]
    async fn overflow_writes_extra_record_directly_then_flushes_buffer() {
        let db = test_db().await;
        let manager = Arc::new(test_manager(db.clone(), Duration::from_secs(5)));
        let flush_guard = manager.writer.lock_flush_for_test().await;

        for index in 0..DEFAULT_AUDIT_LOG_QUEUE_CAPACITY {
            manager.record(audit_request(index as i64)).await;
        }
        manager.record(audit_request(10_000)).await;

        assert_eq!(audit_log_count(&db).await, 1);
        drop(flush_guard);

        wait_for_audit_log_count(&db, (DEFAULT_AUDIT_LOG_QUEUE_CAPACITY + 1) as u64).await;
        manager.cancel();
    }

    #[tokio::test]
    async fn delayed_batch_follows_a_pending_immediate_flush() {
        let db = test_db().await;
        let manager = Arc::new(test_manager(db.clone(), Duration::from_millis(20)));
        let flush_guard = manager.writer.lock_flush_for_test().await;

        for index in 0..DEFAULT_AUDIT_LOG_BATCH_SIZE {
            manager.record(audit_request(index as i64)).await;
        }
        manager
            .record(audit_request(DEFAULT_AUDIT_LOG_BATCH_SIZE as i64))
            .await;
        assert_eq!(audit_log_count(&db).await, 0);

        drop(flush_guard);
        wait_for_audit_log_count(&db, (DEFAULT_AUDIT_LOG_BATCH_SIZE + 1) as u64).await;
        manager.cancel();
    }
}
