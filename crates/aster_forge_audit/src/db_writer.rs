//! Explicitly owned audit persistence using the product-neutral DB store.

use std::sync::Arc;
use std::time::Duration;

use aster_forge_db::{AuditLogCreate, DbError, create_audit_log_requests};
use aster_forge_runtime::{
    BufferFlushError, BufferStopReport, BufferedBatchConfig, BufferedWriterEvent,
    BufferedWriterSnapshot, BufferedWriterState, ManagedBatchWriter, OverflowPolicy,
    RecordAdmission, RecordError, RuntimeComponentBundle, RuntimeComponentBundleRegistration,
    RuntimeComponentKind, RuntimeComponentRegistry, StartupPhaseFailurePolicy, WriteFailure,
    WriteFailureClass, runtime_component,
};
use sea_orm::DatabaseConnection;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::{AUDIT_LOGS_COMPONENT, AUDIT_MANAGER_COMPONENT, AUDIT_MANAGER_FLUSH_SHUTDOWN_PHASE};

/// Default queue capacity; an additional single overflow record can be pending.
pub const DEFAULT_AUDIT_LOG_QUEUE_CAPACITY: usize = 4096;
/// Default immediate-flush threshold.
pub const DEFAULT_AUDIT_LOG_BATCH_SIZE: usize = 100;
/// Default partial-batch delay.
pub const DEFAULT_AUDIT_LOG_DELAYED_FLUSH_AFTER: Duration = Duration::from_secs(1);
/// Explicit writer startup phase, before product lifecycle audit hooks.
pub const AUDIT_WRITER_START_PHASE: &str = "audit_writer_start";

/// Product-neutral audit buffering configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditLogBufferConfig {
    /// Maximum unacknowledged queued records, including an in-flight batch; zero becomes one.
    pub queue_capacity: usize,
    /// Immediate-flush threshold and maximum transaction chunk size; zero becomes one.
    pub batch_size: usize,
    /// Maximum wait from first queued record before a partial batch is attempted.
    pub delayed_flush_after: Duration,
}

impl AuditLogBufferConfig {
    /// Creates bounded audit buffering settings.
    #[must_use]
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

/// Caller-owned audit writer. Construction starts no workers, timers, or database operations.
///
/// Queued admission acknowledges memory only. Mandatory audit should use the DB store in the
/// business transaction, or explicitly await `flush` and map failures at the product boundary.
/// A failed or interrupted write remains available through `unconfirmed`; never replay an unknown
/// commit without reconciling it first. Stop/join this writer before closing the database.
pub struct AuditWriter {
    writer: ManagedBatchWriter<AuditLogCreate, DbError>,
}

impl AuditWriter {
    /// Uses default bounds and a bounded direct-overflow lane.
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        Self::with_config(
            db,
            AuditLogBufferConfig::default(),
            OverflowPolicy::DirectWrite,
            None,
        )
    }

    /// Configures overflow and optional bounded event delivery. Observer code is never invoked
    /// by this writer; a slow, panicking, or absent event consumer cannot block persistence.
    #[must_use]
    pub fn with_config(
        db: DatabaseConnection,
        config: AuditLogBufferConfig,
        overflow: OverflowPolicy,
        events: Option<mpsc::Sender<BufferedWriterEvent>>,
    ) -> Self {
        let writer = ManagedBatchWriter::new(
            BufferedBatchConfig::new(
                config.queue_capacity,
                config.batch_size,
                config.delayed_flush_after,
            ),
            overflow,
            events,
            move |requests: Vec<AuditLogCreate>| {
                let db = db.clone();
                async move {
                    // A single transaction per chunk makes failure atomic and exposes uncertain
                    // commit outcomes. Disable internal retries: the owner decides whether to retry.
                    let retry = aster_forge_db::retry::RetryConfig {
                        max_retries: 0,
                        ..aster_forge_db::retry::RetryConfig::deadlock()
                    };
                    aster_forge_db::transaction::with_transaction_retry(
                        &db,
                        &retry,
                        move |txn| {
                            let requests = requests.clone();
                            Box::pin(async move { create_audit_log_requests(txn, requests).await })
                        },
                        DbError::is_retryable,
                    )
                    .await
                    .map_err(classify_failure)
                }
            },
        );
        Self { writer }
    }

    /// Starts the owned worker exactly once; normally called by the runtime component.
    ///
    /// # Errors
    /// Returns the current state when already started, draining, or stopped.
    pub async fn start(&self) -> Result<(), BufferedWriterState> {
        self.writer.start().await
    }
    /// Admits an audit record. `Queued` acknowledges memory only; `DirectPersisted` acknowledges
    /// just this overflow record. A cancelled direct waiter does not revoke accepted admission.
    ///
    /// # Errors
    /// Rejected records are returned. Direct failures retain the accepted record in this writer.
    pub async fn record(
        &self,
        request: AuditLogCreate,
    ) -> Result<RecordAdmission, RecordError<AuditLogCreate, DbError>> {
        self.writer.record(request).await
    }
    /// Atomically closes admission while keeping accepted records available for drain.
    /// Call after all product producers and the server-shutdown audit hook have finished.
    pub fn begin_drain(&self) {
        self.writer.begin_drain();
    }
    /// Awaits persistence of records accepted before this future's first poll, without closing
    /// admission. Batch and direct writes are acknowledged separately; later records and their
    /// failures do not extend this scope. An expired deadline schedules no new forced write,
    /// while an already acknowledged scope succeeds immediately, even with an expired deadline.
    /// Failed records stay in memory; cancelling this future leaves persistence owned by the worker.
    ///
    /// # Errors
    /// Returns lifecycle, original classified persistence, or deadline errors.
    pub async fn flush(&self, deadline: Instant) -> Result<(), BufferFlushError<DbError>> {
        self.writer.flush(deadline).await
    }
    /// Closes admission, attempts drain, then cancels and joins the worker before returning.
    /// Inspect remaining counts/failures before deciding product exit behavior. Repeated and
    /// concurrent calls are safe; database handles must remain open until this completes.
    pub async fn stop(&self, deadline: Instant) -> BufferStopReport<DbError> {
        self.writer.stop(deadline).await
    }
    /// Explicitly resumes only retry-safe failures; unknown commits and permanent failures stay
    /// blocked. Does not restart a stopped writer or silently replay retained records.
    pub fn retry_failed(&self) {
        self.writer.retry_failed();
    }
    /// Returns per-instance state, acknowledgment counters, and original classified failures.
    #[must_use]
    pub fn snapshot(&self) -> BufferedWriterSnapshot<DbError> {
        self.writer.snapshot()
    }
    /// Copies unacknowledged records for product-controlled reconciliation. Stop the writer
    /// first for a stable recovery snapshot; these records must not be assumed absent from DB.
    #[must_use]
    pub fn unconfirmed(&self) -> Vec<AuditLogCreate> {
        self.writer.unconfirmed()
    }
}

fn classify_failure(error: DbError) -> WriteFailure<DbError> {
    let class = if error.commit_outcome_is_unknown() {
        WriteFailureClass::UnknownCommit
    } else if error.is_retryable() {
        WriteFailureClass::Retryable
    } else {
        WriteFailureClass::Permanent
    };
    WriteFailure::new(class, error)
}

/// Registers explicit writer startup and drain/stop on the existing audit graph.
///
/// Register this bundle BEFORE lifecycle audit hooks so startup can record through the writer.
/// `audit_manager` stops after `audit_logs` (and its producer/mail dependencies); database close
/// must depend on `AUDIT_MANAGER_COMPONENT`. The timeout belongs to the writer, so the runtime
/// must not externally time out this phase before it joins its worker.
/// Drain failures are reported as failed runtime phases, independently of lifecycle-hook policy.
/// An unrepresentable timeout stops immediately and reports a configuration error after joining.
pub fn audit_writer_component(
    writer: Arc<AuditWriter>,
    drain_timeout: Duration,
) -> RuntimeComponentBundleRegistration<impl RuntimeComponentBundle> {
    runtime_component(move |registry: &mut RuntimeComponentRegistry| {
        let startup_writer = writer.clone();
        registry
            .component(AUDIT_MANAGER_COMPONENT)
            .kind(RuntimeComponentKind::Product)
            .depends_on(AUDIT_LOGS_COMPONENT)
            .startup(
                AUDIT_WRITER_START_PHASE,
                StartupPhaseFailurePolicy::Required,
                move || {
                    let writer = startup_writer.clone();
                    async move {
                        writer
                            .start()
                            .await
                            .map_err(|state| format!("audit writer cannot start from {state:?}"))
                    }
                },
            )
            .shutdown(AUDIT_MANAGER_FLUSH_SHUTDOWN_PHASE, None, move || {
                let writer = writer.clone();
                async move { stop_writer(&writer, drain_timeout).await }
            });
    })
}

// The component's timeout is internal: dropping an externally timed phase could close the
// database while its worker is still alive. Even an unrepresentable deadline must stop/join.
async fn stop_writer(writer: &AuditWriter, timeout: Duration) -> Result<(), String> {
    let now = Instant::now();
    let deadline = now.checked_add(timeout);
    let report = writer.stop(deadline.unwrap_or(now)).await;
    let remaining = report.snapshot.queued + report.snapshot.direct_pending;
    if deadline.is_none() {
        return Err(format!(
            "audit drain timeout exceeds clock range; remaining={remaining}; worker joined, database dependency may close"
        ));
    }
    if remaining == 0 {
        return Ok(());
    }
    Err(format!(
        "audit drain incomplete: remaining={remaining}, deadline_elapsed={}, batch_failure={:?}, direct_failure={:?}; worker joined, database dependency may close; reconcile retained records before replay",
        report.deadline_elapsed, report.snapshot.batch_failure, report.snapshot.direct_failure
    ))
}

/// Composes owned writer startup, product lifecycle hooks, and mandatory shutdown drain.
///
/// Unlike a manual collection of hooks, this bundle always registers writer startup first.
/// Lifecycle records keep their product-defined semantics and error policy. With
/// `mail-outbox-dependency`, mail drain finishes before the shutdown record is admitted.
/// Do not also register `audit_component`/`audit_manager_component`, which own the same phase.
pub fn audit_component_with_writer<T, StartFn, StartFut, ShutdownFn, ShutdownFut>(
    resources: T,
    writer: Arc<AuditWriter>,
    drain_timeout: Duration,
    record_server_start: StartFn,
    record_server_shutdown: ShutdownFn,
) -> RuntimeComponentBundleRegistration<impl RuntimeComponentBundle>
where
    T: Clone + Send + 'static,
    StartFn: FnOnce(T) -> StartFut + Send + Sync + 'static,
    StartFut: std::future::Future<Output = Result<(), String>> + Send + 'static,
    ShutdownFn: FnOnce(T) -> ShutdownFut + Send + Sync + 'static,
    ShutdownFut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    audit_component_with_writer_after(
        resources,
        writer,
        drain_timeout,
        crate::DEFAULT_SERVER_SHUTDOWN_AUDIT_DEPENDENCIES,
        record_server_start,
        record_server_shutdown,
    )
}

/// Owned audit bundle with explicit producer dependencies, for example a follower with tasks
/// but no mail outbox. Writer startup still precedes product hooks; stop follows every declared
/// producer. Use this rather than registering placeholder components to satisfy default edges.
pub fn audit_component_with_writer_after<T, StartFn, StartFut, ShutdownFn, ShutdownFut>(
    resources: T,
    writer: Arc<AuditWriter>,
    drain_timeout: Duration,
    shutdown_dependencies: &'static [&'static str],
    record_server_start: StartFn,
    record_server_shutdown: ShutdownFn,
) -> RuntimeComponentBundleRegistration<impl RuntimeComponentBundle>
where
    T: Clone + Send + 'static,
    StartFn: FnOnce(T) -> StartFut + Send + Sync + 'static,
    StartFut: std::future::Future<Output = Result<(), String>> + Send + 'static,
    ShutdownFn: FnOnce(T) -> ShutdownFut + Send + Sync + 'static,
    ShutdownFut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    let owner = audit_writer_component(writer, drain_timeout);
    let start = crate::server_start_audit_component(resources.clone(), record_server_start);
    let shutdown = crate::server_shutdown_audit_component_after(
        resources,
        shutdown_dependencies,
        record_server_shutdown,
    );
    runtime_component(move |registry: &mut RuntimeComponentRegistry| {
        registry
            .register_bundle(owner)
            .register_bundle(start)
            .register_bundle(shutdown);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aster_forge_db::DatabaseErrorKind;
    use aster_forge_runtime::WriteFailureCause;

    #[test]
    fn retains_native_classification_and_unknown_commit_has_priority() {
        let retry = classify_failure(DbError::database_operation_classified(
            "deadlock",
            DatabaseErrorKind::Deadlock,
        ));
        assert_eq!(retry.class, WriteFailureClass::Retryable);
        let unknown = classify_failure(DbError::commit_outcome_unknown(
            "response lost",
            Some(DatabaseErrorKind::Deadlock),
        ));
        assert_eq!(unknown.class, WriteFailureClass::UnknownCommit);
        let WriteFailureCause::Persistence(error) = unknown.cause else {
            panic!("original error missing")
        };
        assert!(error.commit_outcome_is_unknown());
        assert_eq!(
            error.database_error_kind(),
            Some(DatabaseErrorKind::Deadlock)
        );
        assert_eq!(
            classify_failure(DbError::non_retryable("invalid record")).class,
            WriteFailureClass::Permanent
        );
    }
}
