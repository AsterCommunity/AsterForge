//! `SQLite` and `PostgreSQL` exercise the same public persistence and runtime contracts.
#![cfg(feature = "db-writer")]

use aster_forge_audit::*;
use aster_forge_db::{AuditLogCreate, audit_log};
use aster_forge_runtime::*;
use chrono::Utc;
use sea_orm::{ConnectionTrait, DatabaseConnection, EntityTrait, PaginatorTrait};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

fn request(id: i64) -> AuditLogCreate {
    AuditLogCreate {
        user_id: 42,
        action: "test_action".into(),
        entity_type: "test_entity".into(),
        entity_id: Some(id),
        entity_name: None,
        details: None,
        ip_address: None,
        user_agent: None,
        created_at: Utc::now(),
    }
}
fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(5)
}
async fn count(db: &DatabaseConnection) -> u64 {
    audit_log::Entity::find().count(db).await.unwrap()
}
async fn schema(db: &DatabaseConnection) {
    db.execute(&aster_forge_db::create_audit_logs_table(
        db.get_database_backend(),
    ))
    .await
    .unwrap();
}
async fn sqlite() -> DatabaseConnection {
    let db = sea_orm::Database::connect("sqlite::memory:").await.unwrap();
    schema(&db).await;
    db
}
fn writer(db: &DatabaseConnection, capacity: usize, batch: usize, delay: Duration) -> AuditWriter {
    AuditWriter::with_config(
        db.clone(),
        AuditLogBufferConfig::new(capacity, batch, delay),
        OverflowPolicy::DirectWrite,
        None,
    )
}
async fn wait_for_count(db: &DatabaseConnection, expected: u64) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while count(db).await != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

async fn persistence_contract(db: &DatabaseConnection) {
    let writer = writer(db, 3, 2, Duration::from_millis(30));
    assert_eq!(count(db).await, 0);
    assert!(matches!(
        writer.record(request(0)).await,
        Err(RecordError::NotRunning(_))
    ));
    writer.start().await.unwrap();
    writer.record(request(1)).await.unwrap();
    writer.record(request(2)).await.unwrap();
    wait_for_count(db, 2).await;
    writer.record(request(3)).await.unwrap();
    wait_for_count(db, 3).await;
    writer.record(request(4)).await.unwrap();
    writer.flush(deadline()).await.unwrap();
    assert_eq!(count(db).await, 4);
    let (one, two) = tokio::join!(writer.stop(deadline()), writer.stop(deadline()));
    assert_eq!((one.snapshot.persisted, two.snapshot.persisted), (4, 4));
    assert!(writer.unconfirmed().is_empty());
    assert_eq!(writer.start().await, Err(BufferedWriterState::Stopped));
}

#[tokio::test]
async fn sqlite_persistence_and_lifecycle_contract() {
    let db = sqlite().await;
    persistence_contract(&db).await;
    db.close().await.unwrap();
}

#[tokio::test]
async fn full_queue_direct_write_does_not_flush_or_acknowledge_queued_records() {
    let db = sqlite().await;
    let writer = writer(&db, 1, 100, Duration::from_secs(30));
    writer.start().await.unwrap();
    writer.record(request(1)).await.unwrap();
    assert_eq!(
        writer.record(request(2)).await.unwrap(),
        RecordAdmission::DirectPersisted
    );
    assert_eq!(count(&db).await, 1);
    assert_eq!(writer.unconfirmed().len(), 1);
    assert_eq!(writer.unconfirmed()[0].entity_id, Some(1));
    assert_eq!(writer.stop(deadline()).await.snapshot.persisted, 2);
    assert_eq!(count(&db).await, 2);
    db.close().await.unwrap();
}

async fn invalid_batch_contract(db: &DatabaseConnection) {
    let writer = writer(db, 4, 2, Duration::from_secs(30));
    writer.start().await.unwrap();
    let valid = request(1);
    let mut invalid = request(2);
    invalid.action = "x".repeat(65);
    writer.record(valid.clone()).await.unwrap();
    writer.record(invalid.clone()).await.unwrap();
    assert!(matches!(
        writer.flush(deadline()).await,
        Err(BufferFlushError::Write(_))
    ));
    assert_eq!(
        count(db).await,
        0,
        "validation failure must leave the complete transaction uncommitted"
    );
    assert_eq!(writer.unconfirmed(), [valid, invalid]);
    let report = writer.stop(deadline()).await;
    assert_eq!(report.snapshot.queued, 2);
    assert_eq!(report.snapshot.persisted, 0);
    let failure = report.snapshot.batch_failure.unwrap();
    assert_eq!(failure.class, WriteFailureClass::Permanent);
    assert!(matches!(failure.cause, WriteFailureCause::Persistence(_)));
}

#[tokio::test]
async fn validation_failure_is_atomic_and_records_are_retained() {
    let db = sqlite().await;
    invalid_batch_contract(&db).await;
    db.close().await.unwrap();
}

async fn runtime_contract(db: DatabaseConnection, fail: bool) {
    let writer = Arc::new(writer(&db, 4, 100, Duration::from_secs(30)));
    let mut registry = RuntimeComponentRegistry::new();
    let start_writer = writer.clone();
    let shutdown_writer = writer.clone();
    audit_component_with_writer(
        (),
        writer.clone(),
        Duration::from_secs(2),
        move |()| async move {
            start_writer
                .record(request(10))
                .await
                .map(|_| ())
                .map_err(|e| format!("{e:?}"))
        },
        move |()| async move {
            let mut record = request(11);
            if fail {
                record.action = "x".repeat(65);
            }
            shutdown_writer
                .record(record)
                .await
                .map(|_| ())
                .map_err(|e| format!("{e:?}"))
        },
    )
    .register(&mut registry);
    #[cfg(feature = "mail-outbox-dependency")]
    registry
        .component(aster_forge_mail::MAIL_OUTBOX_COMPONENT)
        .shutdown("mail_drain", None, || async { Ok(()) });
    let close_writer = writer.clone();
    registry
        .component("database")
        .depends_on(AUDIT_MANAGER_COMPONENT)
        .shutdown("database_close", None, move || {
            let (db, writer) = (db.clone(), close_writer.clone());
            async move {
                assert_eq!(writer.snapshot().state, BufferedWriterState::Stopped);
                assert_eq!(count(&db).await, if fail { 0 } else { 2 });
                db.close().await.map_err(|e| e.to_string())
            }
        });
    registry.validate().unwrap();
    assert!(!registry.startup().await.aborted());
    assert_eq!(writer.snapshot().state, BufferedWriterState::Running);
    let report = registry.shutdown().await;
    assert_eq!(report.has_failures(), fail);
    let phases: Vec<_> = report.phases.iter().map(|p| p.name).collect();
    let flush = phases
        .iter()
        .position(|p| *p == AUDIT_MANAGER_FLUSH_SHUTDOWN_PHASE)
        .unwrap();
    let close = phases.iter().position(|p| *p == "database_close").unwrap();
    assert!(flush < close);
    assert_eq!(writer.unconfirmed().len(), if fail { 2 } else { 0 });
}

#[tokio::test]
async fn runtime_starts_writer_before_hooks_and_joins_before_database_close() {
    runtime_contract(sqlite().await, false).await;
}
#[tokio::test]
async fn runtime_reports_drain_failure_and_retains_records_before_database_close() {
    runtime_contract(sqlite().await, true).await;
}

#[cfg(feature = "database-container-tests")]
#[tokio::test]
async fn postgres_audit_writer_contract() {
    use aster_forge_test::{postgres::PostgresTestContainer, suite::TestContainerSuite};
    let container =
        PostgresTestContainer::start(&TestContainerSuite::new("asterforge-audit-writer")).await;
    for case in [
        "persistence",
        "failure",
        "runtime",
        "runtime_failure",
        "runtime_service_panic",
        "runtime_cancel_panic",
        "constraint",
        "timeout",
        "retry",
    ] {
        let fixture = container
            .create_database(&format!("audit_{}", uuid::Uuid::new_v4().simple()))
            .await;
        let db = fixture.connect().await;
        schema(&db).await;
        match case {
            "persistence" => persistence_contract(&db).await,
            "failure" => invalid_batch_contract(&db).await,
            "runtime" => runtime_contract(db.clone(), false).await,
            "runtime_failure" => runtime_contract(db.clone(), true).await,
            "runtime_service_panic" => panic_cleanup_contract(db.clone(), "service").await,
            "runtime_cancel_panic" => cancellation_panic_cleanup_contract(db.clone()).await,
            "constraint" => postgres_constraint_failure(&db).await,
            "timeout" => postgres_blocked_drain(&db).await,
            _ => postgres_safe_retry(fixture.url(), &db).await,
        }
        if !case.starts_with("runtime") {
            db.close().await.unwrap();
        }
        fixture.cleanup().await;
    }
}

#[cfg(feature = "database-container-tests")]
async fn postgres_constraint_failure(db: &DatabaseConnection) {
    use aster_forge_db::DatabaseErrorKind;
    db.execute_unprepared("CREATE UNIQUE INDEX audit_entity_unique ON audit_logs (entity_id)")
        .await
        .unwrap();
    let writer = writer(db, 4, 2, Duration::from_secs(30));
    writer.start().await.unwrap();
    writer.record(request(1)).await.unwrap();
    writer.record(request(1)).await.unwrap();
    let Err(BufferFlushError::Write(failure)) = writer.flush(deadline()).await else {
        panic!("expected constraint failure")
    };
    assert_eq!(failure.class, WriteFailureClass::Permanent);
    let WriteFailureCause::Persistence(error) = failure.cause else {
        panic!("lost database error")
    };
    assert_eq!(
        error.database_error_kind(),
        Some(DatabaseErrorKind::UniqueConstraint)
    );
    assert_eq!(count(db).await, 0);
    assert_eq!(writer.stop(deadline()).await.snapshot.queued, 2);
}

#[cfg(feature = "database-container-tests")]
async fn postgres_blocked_drain(db: &DatabaseConnection) {
    use sea_orm::TransactionTrait;
    let lock = db.begin().await.unwrap();
    lock.execute_unprepared("LOCK TABLE audit_logs IN ACCESS EXCLUSIVE MODE")
        .await
        .unwrap();
    let writer = writer(db, 2, 1, Duration::from_secs(30));
    writer.start().await.unwrap();
    writer.record(request(1)).await.unwrap();
    let report = writer
        .stop(Instant::now() + Duration::from_millis(100))
        .await;
    assert!(report.deadline_elapsed);
    assert_eq!(report.snapshot.state, BufferedWriterState::Stopped);
    assert_eq!(report.snapshot.queued, 1);
    assert_eq!(
        report.snapshot.batch_failure.unwrap().class,
        WriteFailureClass::UnknownCommit
    );
    lock.rollback().await.unwrap();
    assert_eq!(
        count(db).await,
        0,
        "cancelled worker must not write after dependency close becomes eligible"
    );
    writer.retry_failed();
    assert_eq!(writer.stop(deadline()).await.snapshot.persisted, 0);
}

#[cfg(feature = "database-container-tests")]
async fn postgres_safe_retry(url: &str, db: &DatabaseConnection) {
    use sea_orm::TransactionTrait;
    // A dedicated one-connection writer pool makes the session lock timeout deterministic.
    let mut options = sea_orm::ConnectOptions::new(url);
    options.max_connections(1);
    let writer_db = sea_orm::Database::connect(options).await.unwrap();
    writer_db
        .execute_unprepared("SET lock_timeout = '50ms'")
        .await
        .unwrap();
    let lock = db.begin().await.unwrap();
    lock.execute_unprepared("LOCK TABLE audit_logs IN ACCESS EXCLUSIVE MODE")
        .await
        .unwrap();
    let writer = writer(&writer_db, 2, 1, Duration::from_secs(30));
    writer.start().await.unwrap();
    writer.record(request(1)).await.unwrap();
    let Err(BufferFlushError::Write(failure)) = writer.flush(deadline()).await else {
        panic!("expected lock timeout")
    };
    assert_eq!(failure.class, WriteFailureClass::Retryable);
    let WriteFailureCause::Persistence(error) = failure.cause else {
        panic!("lost database error")
    };
    assert_eq!(
        error.database_error_kind(),
        Some(aster_forge_db::DatabaseErrorKind::LockTimeout)
    );
    assert_eq!(writer.snapshot().persisted, 0);
    lock.rollback().await.unwrap();
    assert_eq!(count(db).await, 0);
    writer.retry_failed();
    writer.flush(deadline()).await.unwrap();
    assert_eq!(writer.stop(deadline()).await.snapshot.persisted, 1);
    assert_eq!(count(db).await, 1);
    writer_db.close().await.unwrap();
}

#[tokio::test]
async fn explicit_producer_dependencies_override_default_mail_edge() {
    let db = sqlite().await;
    let writer = Arc::new(AuditWriter::new(db.clone()));
    let mut registry = RuntimeComponentRegistry::new();
    audit_component_with_writer_after(
        (),
        writer.clone(),
        Duration::from_secs(2),
        &["producer"],
        |()| async { Ok(()) },
        |()| async { Ok(()) },
    )
    .register(&mut registry);
    let producer_writer = writer.clone();
    registry
        .component("producer")
        .shutdown("producer_stop", None, move || {
            let writer = producer_writer.clone();
            async move {
                writer
                    .record(request(1))
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            }
        });
    registry.validate().unwrap();
    assert_eq!(
        registry
            .descriptor(AUDIT_LOGS_COMPONENT)
            .unwrap()
            .dependencies,
        ["producer"]
    );
    assert!(!registry.startup().await.aborted());
    assert!(!registry.shutdown().await.has_failures());
    assert_eq!(writer.snapshot().state, BufferedWriterState::Stopped);
    assert_eq!(count(&db).await, 1);
    db.close().await.unwrap();
}

#[tokio::test]
async fn unrepresentable_component_deadline_reports_failure_after_joining() {
    let db = sqlite().await;
    let writer = Arc::new(AuditWriter::new(db.clone()));
    let mut registry = RuntimeComponentRegistry::new();
    audit_component_with_writer_after(
        (),
        writer.clone(),
        Duration::MAX,
        &[],
        |()| async { Ok(()) },
        |()| async { Ok(()) },
    )
    .register(&mut registry);
    assert!(!registry.startup().await.aborted());
    let report = registry.shutdown().await;
    assert!(report.has_failures());
    assert_eq!(writer.snapshot().state, BufferedWriterState::Stopped);
    assert!(report.phases.iter().any(|phase| matches!(&phase.status, ShutdownPhaseStatus::Failed(error) if error.contains("timeout exceeds clock range") && error.contains("worker joined"))));
    db.close().await.unwrap();
}

#[tokio::test]
async fn runtime_run_propagates_failed_audit_drain_after_database_close() {
    let db = sqlite().await;
    let writer = Arc::new(AuditWriter::new(db.clone()));
    let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let close_writer = writer.clone();
    let close_flag = closed.clone();
    let result = AsterRuntime::builder()
        .component(runtime_component_with_shutdown(|shutdown| {
            RuntimeServiceComponent::new(
                "http",
                RuntimeComponentKind::Core,
                async {},
                shutdown,
                || async {},
            )
        }))
        .component(audit_component_with_writer_after(
            writer.clone(),
            writer.clone(),
            Duration::from_secs(1),
            &[],
            |writer| async move {
                let mut invalid = request(1);
                invalid.action = "x".repeat(65);
                writer
                    .record(invalid)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            },
            |_| async { Ok(()) },
        ))
        .component(runtime_component(
            move |registry: &mut RuntimeComponentRegistry| {
                registry
                    .component("database")
                    .depends_on(AUDIT_MANAGER_COMPONENT)
                    .shutdown("database_close", None, move || {
                        let (db, writer, closed) =
                            (db.clone(), close_writer.clone(), close_flag.clone());
                        async move {
                            assert_eq!(writer.snapshot().state, BufferedWriterState::Stopped);
                            db.close().await.map_err(|error| error.to_string())?;
                            closed.store(true, std::sync::atomic::Ordering::SeqCst);
                            Ok(())
                        }
                    });
            },
        ))
        .run()
        .await;
    let Err(AsterRuntimeError::Shutdown { report }) = result else {
        panic!("runtime must propagate audit drain failure")
    };
    assert!(closed.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(writer.unconfirmed().len(), 1);
    assert!(
        matches!(&report.phases.iter().find(|phase| phase.name == AUDIT_MANAGER_FLUSH_SHUTDOWN_PHASE).unwrap().status, ShutdownPhaseStatus::Failed(error) if error.contains("remaining=1"))
    );
    assert_eq!(report.phases.last().unwrap().name, "database_close");
}

#[tokio::test]
async fn panicking_lifecycle_phases_flush_accepted_audit_before_database_close() {
    for phase in ["startup", "before_shutdown", "service"] {
        panic_cleanup_contract(sqlite().await, phase).await;
    }
}

async fn panic_cleanup_contract(db: DatabaseConnection, phase: &'static str) {
    let startup_panics = phase == "startup";
    let writer = Arc::new(AuditWriter::new(db.clone()));
    let (close_writer, start_writer) = (writer.clone(), writer.clone());
    let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let close_flag = closed.clone();
    let result = AsterRuntime::builder()
        .component(runtime_component_with_shutdown(|shutdown| {
            RuntimeServiceComponent::new(
                "http",
                RuntimeComponentKind::Core,
                async move {
                    assert_ne!(phase, "service", "service panicked");
                },
                shutdown,
                || async {},
            )
        }))
        .component(audit_component_with_writer_after(
            (),
            writer.clone(),
            Duration::from_secs(1),
            &[],
            move |()| async move {
                start_writer
                    .record(request(1))
                    .await
                    .map_err(|error| error.to_string())?;
                assert!(!startup_panics, "product startup audit hook panicked");
                Ok(())
            },
            |()| async { Ok(()) },
        ))
        .before_shutdown(move || async move {
            assert_ne!(
                phase, "before_shutdown",
                "product pre-shutdown hook panicked"
            );
        })
        .component(runtime_component(
            move |registry: &mut RuntimeComponentRegistry| {
                registry
                    .component("database")
                    .depends_on(AUDIT_MANAGER_COMPONENT)
                    .shutdown("database_close", None, move || {
                        let (db, writer, flag) =
                            (db.clone(), close_writer.clone(), close_flag.clone());
                        async move {
                            assert_eq!(writer.snapshot().state, BufferedWriterState::Stopped);
                            assert_eq!(count(&db).await, 1);
                            db.close().await.map_err(|error| error.to_string())?;
                            flag.store(true, std::sync::atomic::Ordering::SeqCst);
                            Ok(())
                        }
                    });
            },
        ))
        .run()
        .await;
    let report = match result {
        Err(AsterRuntimeError::Startup {
            shutdown_report, ..
        }) if startup_panics => shutdown_report,
        Err(AsterRuntimeError::BeforeShutdown { shutdown_report })
            if phase == "before_shutdown" =>
        {
            shutdown_report
        }
        Err(AsterRuntimeError::Service {
            shutdown_report,
            before_shutdown_panicked,
        }) if phase == "service" => {
            assert!(!before_shutdown_panicked);
            shutdown_report
        }
        result => panic!("unexpected runtime result: {result:?}"),
    };
    assert!(!report.has_failures());
    assert!(closed.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(writer.snapshot().persisted, 1);
    assert!(writer.unconfirmed().is_empty());
}

struct PanickingCancellationGuard;
impl Drop for PanickingCancellationGuard {
    fn drop(&mut self) {
        panic!("product cancellation cleanup panicked");
    }
}

#[tokio::test]
async fn producer_timeout_destructor_panic_still_drains_audit_and_joins_before_database_close() {
    cancellation_panic_cleanup_contract(sqlite().await).await;
}

async fn cancellation_panic_cleanup_contract(db: DatabaseConnection) {
    let writer = Arc::new(AuditWriter::new(db.clone()));
    let (start_writer, shutdown_writer, close_writer) =
        (writer.clone(), writer.clone(), writer.clone());
    let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let close_flag = closed.clone();
    let result = AsterRuntime::builder()
        .component(runtime_component_with_shutdown(|shutdown| {
            RuntimeServiceComponent::new(
                "http",
                RuntimeComponentKind::Core,
                async {},
                shutdown,
                || async {},
            )
        }))
        .component(runtime_component(
            |registry: &mut RuntimeComponentRegistry| {
                registry.component("producer").shutdown(
                    "producer_stop",
                    Some(Duration::from_millis(1)),
                    || async {
                        let _guard = PanickingCancellationGuard;
                        std::future::pending().await
                    },
                );
            },
        ))
        .component(audit_component_with_writer_after(
            (),
            writer.clone(),
            Duration::from_secs(2),
            &["producer"],
            move |()| async move {
                start_writer
                    .record(request(1))
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            },
            move |()| async move {
                shutdown_writer
                    .record(request(2))
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            },
        ))
        .component(runtime_component(
            move |registry: &mut RuntimeComponentRegistry| {
                registry
                    .component("database")
                    .depends_on(AUDIT_MANAGER_COMPONENT)
                    .shutdown("database_close", None, move || {
                        let (db, writer, flag) =
                            (db.clone(), close_writer.clone(), close_flag.clone());
                        async move {
                            assert_eq!(writer.snapshot().state, BufferedWriterState::Stopped);
                            assert_eq!(count(&db).await, 2);
                            db.close().await.map_err(|error| error.to_string())?;
                            flag.store(true, std::sync::atomic::Ordering::SeqCst);
                            Ok(())
                        }
                    });
            },
        ))
        .run()
        .await;
    let Err(AsterRuntimeError::Shutdown { report }) = result else {
        panic!("producer cancellation panic must reach the caller after all cleanup")
    };
    assert_eq!(report.phases[0].name, "producer_stop");
    assert_eq!(
        report.phases[0].status,
        ShutdownPhaseStatus::Failed("shutdown phase panicked".into())
    );
    assert!(
        report.phases[1..]
            .iter()
            .all(|phase| phase.status == ShutdownPhaseStatus::Succeeded)
    );
    assert_eq!(report.phases.last().unwrap().name, "database_close");
    assert!(closed.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(writer.snapshot().persisted, 2);
    assert!(writer.unconfirmed().is_empty());
}
