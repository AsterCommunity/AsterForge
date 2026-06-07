use chrono::Utc;
use sea_orm::{DatabaseConnection, Set};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::context::AuditContext;
use crate::config::RuntimeConfig;
use crate::db::repository::audit_log_repo;
use crate::entities::audit_log;
use crate::runtime::SharedRuntimeState;
use crate::types::{AuditAction, AuditEntityType};

const AUDIT_LOG_QUEUE_CAPACITY: usize = 4096;
const AUDIT_LOG_BATCH_SIZE: usize = 100;
const AUDIT_LOG_DELAYED_FLUSH_AFTER: Duration = Duration::from_secs(1);

static GLOBAL_AUDIT_LOG_MANAGER: OnceLock<Arc<AuditLogManager>> = OnceLock::new();

struct AuditLogManager {
    db: DatabaseConnection,
    buffer: parking_lot::Mutex<Vec<audit_log::ActiveModel>>,
    flush_lock: Mutex<()>,
    flush_pending: AtomicBool,
    delayed_flush_pending: AtomicBool,
    delayed_flush_after: Duration,
    shutdown_token: CancellationToken,
}

struct FlushPendingReset {
    manager: Arc<AuditLogManager>,
    armed: bool,
}

impl Drop for FlushPendingReset {
    fn drop(&mut self) {
        if self.armed {
            self.manager.flush_pending.store(false, Ordering::Release);
        }
    }
}

impl FlushPendingReset {
    fn reset(&mut self) {
        self.manager.flush_pending.store(false, Ordering::Release);
        self.armed = false;
    }
}

struct DelayedFlushPendingReset {
    manager: Arc<AuditLogManager>,
    armed: bool,
}

impl Drop for DelayedFlushPendingReset {
    fn drop(&mut self) {
        if self.armed {
            self.manager
                .delayed_flush_pending
                .store(false, Ordering::Release);
        }
    }
}

impl DelayedFlushPendingReset {
    fn reset(&mut self) {
        self.manager
            .delayed_flush_pending
            .store(false, Ordering::Release);
        self.armed = false;
    }
}

impl AuditLogManager {
    fn new(db: DatabaseConnection) -> Self {
        Self::new_with_delayed_flush_after(db, AUDIT_LOG_DELAYED_FLUSH_AFTER)
    }

    fn new_with_delayed_flush_after(db: DatabaseConnection, delayed_flush_after: Duration) -> Self {
        Self {
            db,
            buffer: parking_lot::Mutex::new(Vec::with_capacity(AUDIT_LOG_BATCH_SIZE)),
            flush_lock: Mutex::new(()),
            flush_pending: AtomicBool::new(false),
            delayed_flush_pending: AtomicBool::new(false),
            delayed_flush_after,
            shutdown_token: CancellationToken::new(),
        }
    }

    async fn record(self: &Arc<Self>, model: audit_log::ActiveModel) {
        let mut overflow_model = None;
        let should_flush;
        let should_schedule_delayed_flush;
        {
            let mut buffer = self.buffer.lock();
            if buffer.len() >= AUDIT_LOG_QUEUE_CAPACITY {
                overflow_model = Some(model);
                should_flush = false;
                should_schedule_delayed_flush = false;
            } else {
                let was_empty = buffer.is_empty();
                buffer.push(model);
                should_flush = buffer.len() >= AUDIT_LOG_BATCH_SIZE;
                should_schedule_delayed_flush = !should_flush && was_empty;
            }
        }

        if let Some(model) = overflow_model {
            tracing::warn!(
                capacity = AUDIT_LOG_QUEUE_CAPACITY,
                "audit log buffer is full; falling back to direct write"
            );
            self.schedule_flush();
            write_audit_model(&self.db, model).await;
            return;
        }

        if should_flush {
            self.schedule_flush();
        } else if should_schedule_delayed_flush {
            self.schedule_delayed_flush();
        }
    }

    fn schedule_flush(self: &Arc<Self>) {
        if self
            .flush_pending
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        let manager = Arc::clone(self);
        drop(tokio::spawn(async move {
            let mut pending_reset = FlushPendingReset {
                manager: Arc::clone(&manager),
                armed: true,
            };
            {
                let _guard = manager.flush_lock.lock().await;
                manager.flush_buffer().await;
            }
            pending_reset.reset();
            manager.schedule_buffered_flush();
        }));
    }

    fn schedule_delayed_flush(self: &Arc<Self>) {
        if self
            .delayed_flush_pending
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        let manager = Arc::clone(self);
        drop(tokio::spawn(async move {
            let mut pending_reset = DelayedFlushPendingReset {
                manager: Arc::clone(&manager),
                armed: true,
            };
            let delayed_flush_after = manager.delayed_flush_after;
            tokio::select! {
                biased;
                _ = manager.shutdown_token.cancelled() => return,
                _ = tokio::time::sleep(delayed_flush_after) => {}
            }

            {
                let _guard = manager.flush_lock.lock().await;
                manager.flush_buffer().await;
            }
            pending_reset.reset();
            manager.schedule_buffered_flush();
        }));
    }

    fn schedule_buffered_flush(self: &Arc<Self>) {
        let buffered_count = self.buffer.lock().len();
        if buffered_count >= AUDIT_LOG_BATCH_SIZE {
            self.schedule_flush();
        } else if buffered_count > 0 {
            self.schedule_delayed_flush();
        }
    }

    async fn flush(self: &Arc<Self>) {
        let _guard = self.flush_lock.lock().await;
        self.flush_buffer().await;
        if self.buffer.lock().is_empty() {
            self.flush_pending.store(false, Ordering::Release);
            self.delayed_flush_pending.store(false, Ordering::Release);
        }
        self.schedule_buffered_flush();
    }

    fn cancel(&self) {
        self.shutdown_token.cancel();
    }

    #[cfg(test)]
    async fn lock_flush_for_test(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.flush_lock.lock().await
    }

    async fn flush_buffer(&self) {
        let mut models = {
            let mut buffer = self.buffer.lock();
            if buffer.is_empty() {
                return;
            }
            std::mem::take(&mut *buffer)
        };
        write_audit_batch(&self.db, &mut models).await;
    }
}

pub fn init_global_audit_log_manager(db: DatabaseConnection) {
    let manager = Arc::new(AuditLogManager::new(db));
    if GLOBAL_AUDIT_LOG_MANAGER.set(manager).is_err() {
        tracing::warn!("global audit log manager is already initialized; ignoring");
    }
}

pub async fn flush_global_audit_log_manager() {
    if let Some(manager) = GLOBAL_AUDIT_LOG_MANAGER.get() {
        manager.flush().await;
    }
}

pub async fn shutdown_global_audit_log_manager() {
    if let Some(manager) = GLOBAL_AUDIT_LOG_MANAGER.get() {
        manager.cancel();
        manager.flush().await;
    }
}

async fn write_audit_model(db: &DatabaseConnection, model: audit_log::ActiveModel) {
    if let Err(error) = audit_log_repo::create(db, model).await {
        tracing::warn!("failed to write audit log: {error}");
    }
}

async fn write_audit_batch(db: &DatabaseConnection, batch: &mut Vec<audit_log::ActiveModel>) {
    if batch.is_empty() {
        return;
    }

    let total = batch.len();
    let mut models = std::mem::take(batch).into_iter();
    loop {
        let chunk = models
            .by_ref()
            .take(AUDIT_LOG_BATCH_SIZE)
            .collect::<Vec<_>>();
        if chunk.is_empty() {
            break;
        }

        let count = chunk.len();
        if let Err(error) = audit_log_repo::create_many(db, chunk).await {
            tracing::warn!(count, total, "failed to write audit log batch: {error}");
        }
    }
}

pub fn should_record<S: SharedRuntimeState>(state: &S, action: AuditAction) -> bool {
    state.runtime_config().should_record_audit_action(action)
}

pub fn should_record_with_config(runtime_config: &RuntimeConfig, action: AuditAction) -> bool {
    runtime_config.should_record_audit_action(action)
}

fn audit_model(
    ctx: &AuditContext,
    action: AuditAction,
    entity_type: AuditEntityType,
    entity_id: Option<i64>,
    entity_name: Option<&str>,
    details: Option<serde_json::Value>,
) -> audit_log::ActiveModel {
    audit_log::ActiveModel {
        id: Default::default(),
        user_id: Set(ctx.user_id),
        action: Set(action),
        entity_type: Set(entity_type.as_str().to_string()),
        entity_id: Set(entity_id),
        entity_name: Set(entity_name.map(ToOwned::to_owned)),
        details: Set(details.map(|value| value.to_string())),
        ip_address: Set(ctx.ip_address.clone()),
        user_agent: Set(ctx.user_agent.clone()),
        created_at: Set(Utc::now()),
    }
}

async fn record_prechecked<S: SharedRuntimeState>(
    state: &S,
    ctx: &AuditContext,
    action: AuditAction,
    entity_type: AuditEntityType,
    entity_id: Option<i64>,
    entity_name: Option<&str>,
    details: Option<serde_json::Value>,
) {
    let model = audit_model(ctx, action, entity_type, entity_id, entity_name, details);

    if let Some(manager) = GLOBAL_AUDIT_LOG_MANAGER.get() {
        manager.record(model).await;
    } else {
        write_audit_model(state.writer_db(), model).await;
    }
}

async fn record_prechecked_with_db(
    db: &DatabaseConnection,
    ctx: &AuditContext,
    action: AuditAction,
    entity_type: AuditEntityType,
    entity_id: Option<i64>,
    entity_name: Option<&str>,
    details: Option<serde_json::Value>,
) {
    let model = audit_model(ctx, action, entity_type, entity_id, entity_name, details);
    write_audit_model(db, model).await;
}

pub async fn log<S: SharedRuntimeState>(
    state: &S,
    ctx: &AuditContext,
    action: AuditAction,
    entity_type: AuditEntityType,
    entity_id: Option<i64>,
    entity_name: Option<&str>,
    details: Option<serde_json::Value>,
) {
    if !should_record(state, action) {
        return;
    }

    record_prechecked(
        state,
        ctx,
        action,
        entity_type,
        entity_id,
        entity_name,
        details,
    )
    .await;
}

pub async fn log_with_db_and_config<F>(
    db: &DatabaseConnection,
    runtime_config: &RuntimeConfig,
    ctx: &AuditContext,
    action: AuditAction,
    entity_type: AuditEntityType,
    entity_id: Option<i64>,
    entity_name: Option<&str>,
    details: F,
) where
    F: FnOnce() -> Option<serde_json::Value>,
{
    if !should_record_with_config(runtime_config, action) {
        return;
    }

    record_prechecked_with_db(
        db,
        ctx,
        action,
        entity_type,
        entity_id,
        entity_name,
        details(),
    )
    .await;
}

pub async fn log_with_details<S, F>(
    state: &S,
    ctx: &AuditContext,
    action: AuditAction,
    entity_type: AuditEntityType,
    entity_id: Option<i64>,
    entity_name: Option<&str>,
    details: F,
) where
    S: SharedRuntimeState,
    F: FnOnce() -> Option<serde_json::Value>,
{
    if !should_record(state, action) {
        return;
    }

    record_prechecked(
        state,
        ctx,
        action,
        entity_type,
        entity_id,
        entity_name,
        details(),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::{
        AUDIT_LOG_BATCH_SIZE, AUDIT_LOG_QUEUE_CAPACITY, AuditLogManager, write_audit_batch,
    };
    use crate::config::DatabaseConfig;
    use crate::entities::audit_log;
    use crate::types::{AuditAction, AuditEntityType};
    use chrono::Utc;
    use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    async fn build_test_db() -> sea_orm::DatabaseConnection {
        let db = crate::db::connect_with_metrics(
            &DatabaseConfig {
                url: "sqlite::memory:".to_string(),
                pool_size: 1,
                retry_count: 0,
            },
            crate::metrics_core::NoopMetrics::arc(),
        )
        .await
        .expect("audit manager test DB should connect");
        migration::Migrator::up(&db, None)
            .await
            .expect("audit manager test migrations should succeed");
        db
    }

    fn audit_model(index: i64) -> audit_log::ActiveModel {
        audit_log::ActiveModel {
            id: Default::default(),
            user_id: Set(42),
            action: Set(AuditAction::ConfigUpdate),
            entity_type: Set(AuditEntityType::SystemConfig.as_str().to_string()),
            entity_id: Set(Some(index)),
            entity_name: Set(Some(format!("config-{index}"))),
            details: Set(Some(serde_json::json!({ "index": index }).to_string())),
            ip_address: Set(Some("127.0.0.1".to_string())),
            user_agent: Set(Some("audit-manager-test".to_string())),
            created_at: Set(Utc::now()),
        }
    }

    async fn audit_log_count(db: &sea_orm::DatabaseConnection) -> u64 {
        audit_log::Entity::find()
            .filter(audit_log::Column::Action.eq(AuditAction::ConfigUpdate))
            .count(db)
            .await
            .expect("audit manager count query should succeed")
    }

    async fn wait_for_audit_log_count(db: &sea_orm::DatabaseConnection, expected: u64) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let current = audit_log_count(db).await;
            if current == expected {
                return;
            }
            assert!(
                current < expected,
                "audit log count exceeded expected value: expected {expected}, got {current}"
            );
            assert!(
                Instant::now() < deadline,
                "timed out waiting for audit log count {expected}; last count was {current}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn manager_flushes_threshold_batch() {
        let db = build_test_db().await;
        let manager = Arc::new(AuditLogManager::new_with_delayed_flush_after(
            db.clone(),
            Duration::from_secs(5),
        ));

        for index in 0..AUDIT_LOG_BATCH_SIZE {
            manager.record(audit_model(index as i64)).await;
        }

        wait_for_audit_log_count(&db, AUDIT_LOG_BATCH_SIZE as u64).await;
        manager.cancel();
        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn manager_flushes_single_log_after_delay() {
        let db = build_test_db().await;
        let manager = Arc::new(AuditLogManager::new_with_delayed_flush_after(
            db.clone(),
            Duration::from_millis(20),
        ));

        manager.record(audit_model(1)).await;

        wait_for_audit_log_count(&db, 1).await;
        manager.cancel();
        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn manager_flushes_buffer_on_cancelled_shutdown() {
        let db = build_test_db().await;
        let manager = Arc::new(AuditLogManager::new_with_delayed_flush_after(
            db.clone(),
            Duration::from_secs(5),
        ));

        manager.record(audit_model(1)).await;
        manager.cancel();
        manager.flush().await;

        assert_eq!(audit_log_count(&db).await, 1);
        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn manager_manual_flush_allows_later_delayed_flush() {
        let db = build_test_db().await;
        let manager = Arc::new(AuditLogManager::new_with_delayed_flush_after(
            db.clone(),
            Duration::from_millis(20),
        ));

        manager.record(audit_model(1)).await;
        manager.flush().await;
        assert_eq!(audit_log_count(&db).await, 1);

        manager.record(audit_model(2)).await;
        wait_for_audit_log_count(&db, 2).await;

        manager.cancel();
        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn manager_cancel_stops_delayed_flush_until_explicit_flush() {
        let db = build_test_db().await;
        let manager = Arc::new(AuditLogManager::new_with_delayed_flush_after(
            db.clone(),
            Duration::from_millis(20),
        ));

        manager.record(audit_model(1)).await;
        manager.cancel();
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(audit_log_count(&db).await, 0);

        manager.flush().await;
        assert_eq!(audit_log_count(&db).await, 1);
        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn manager_overflow_writes_extra_log_directly_and_flushes_buffer() {
        let db = build_test_db().await;
        let manager = Arc::new(AuditLogManager::new_with_delayed_flush_after(
            db.clone(),
            Duration::from_secs(5),
        ));
        let flush_guard = manager.lock_flush_for_test().await;

        for index in 0..AUDIT_LOG_QUEUE_CAPACITY {
            manager.record(audit_model(index as i64)).await;
        }
        manager.record(audit_model(10_000)).await;

        assert_eq!(audit_log_count(&db).await, 1);
        drop(flush_guard);

        wait_for_audit_log_count(&db, (AUDIT_LOG_QUEUE_CAPACITY + 1) as u64).await;
        manager.cancel();
        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn write_audit_batch_splits_large_batches_and_drains_input() {
        let db = build_test_db().await;
        let mut batch = (0..(AUDIT_LOG_BATCH_SIZE + 7))
            .map(|index| audit_model(index as i64))
            .collect::<Vec<_>>();

        write_audit_batch(&db, &mut batch).await;

        assert!(batch.is_empty());
        assert_eq!(
            audit_log_count(&db).await,
            (AUDIT_LOG_BATCH_SIZE + 7) as u64
        );
        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn write_audit_batch_ignores_empty_input() {
        let db = build_test_db().await;
        let mut batch = Vec::new();

        write_audit_batch(&db, &mut batch).await;

        assert_eq!(audit_log_count(&db).await, 0);
        db.close().await.unwrap();
    }
}
