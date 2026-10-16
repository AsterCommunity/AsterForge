//! Runtime errors retain shutdown failures after all dependency cleanup has completed.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use aster_forge_runtime::*;
use tokio_util::sync::CancellationToken;

#[tokio::test(start_paused = true)]
async fn shutdown_timeout_reaches_caller_after_dependent_cleanup() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let result = AsterRuntime::builder()
        .component(RuntimeServiceComponent::new(
            "service",
            RuntimeComponentKind::Core,
            async {},
            CancellationToken::default(),
            || async {},
        ))
        .component(runtime_component(
            move |registry: &mut RuntimeComponentRegistry| {
                registry.component("writer").shutdown(
                    "writer_stop",
                    Some(Duration::from_secs(1)),
                    std::future::pending,
                );
                registry
                    .component("database")
                    .depends_on("writer")
                    .shutdown("database_close", None, move || {
                        observed.lock().unwrap().push("database_close");
                        async { Ok(()) }
                    });
            },
        ))
        .run()
        .await;
    let Err(AsterRuntimeError::Shutdown { report }) = result else {
        panic!("shutdown timeout must reach the runtime caller")
    };
    assert_eq!(report.phases[0].status, ShutdownPhaseStatus::TimedOut);
    assert_eq!(report.phases[1].status, ShutdownPhaseStatus::Succeeded);
    assert_eq!(*events.lock().unwrap(), ["database_close"]);
}

#[tokio::test]
async fn startup_error_retains_secondary_shutdown_error_and_cleanup_order() {
    let result = AsterRuntime::builder()
        .component(RuntimeServiceComponent::new(
            "service",
            RuntimeComponentKind::Core,
            std::future::pending::<()>(),
            CancellationToken::default(),
            || async {},
        ))
        .component(runtime_component(
            |registry: &mut RuntimeComponentRegistry| {
                registry
                    .component("writer")
                    .startup(
                        "writer_start",
                        StartupPhaseFailurePolicy::Required,
                        || async { Err("startup failed".into()) },
                    )
                    .shutdown("writer_stop", None, || async { Err("drain failed".into()) });
                registry
                    .component("database")
                    .depends_on("writer")
                    .shutdown("database_close", None, || async { Ok(()) });
            },
        ))
        .run()
        .await;
    let Err(AsterRuntimeError::Startup {
        report,
        shutdown_report,
    }) = result
    else {
        panic!("startup and shutdown reports must both be preserved")
    };
    assert!(report.aborted());
    assert_eq!(
        shutdown_report.phases[0].status,
        ShutdownPhaseStatus::Failed("drain failed".into())
    );
    assert_eq!(
        shutdown_report.phases[1].status,
        ShutdownPhaseStatus::Succeeded
    );
    assert_eq!(shutdown_report.phases[1].name, "database_close");
}

#[tokio::test]
async fn panicking_shutdown_hook_is_reported_without_skipping_dependency_cleanup() {
    for synchronous in [true, false] {
        let result = AsterRuntime::builder()
            .component(RuntimeServiceComponent::new(
                "service",
                RuntimeComponentKind::Core,
                async {},
                CancellationToken::default(),
                || async {},
            ))
            .component(runtime_component(
                move |registry: &mut RuntimeComponentRegistry| {
                    registry
                        .component("producer")
                        .shutdown("producer_stop", None, move || {
                            assert!(!synchronous, "callback construction panicked");
                            async { panic!("callback polling panicked") }
                        });
                    registry
                        .component("writer")
                        .depends_on("producer")
                        .shutdown("writer_stop", None, || async { Ok(()) });
                    registry
                        .component("database")
                        .depends_on("writer")
                        .shutdown("database_close", None, || async { Ok(()) });
                },
            ))
            .run()
            .await;
        let Err(AsterRuntimeError::Shutdown { report }) = result else {
            panic!("phase panic must reach caller after cleanup")
        };
        assert_eq!(
            report.phases[0].status,
            ShutdownPhaseStatus::Failed("shutdown phase panicked".into())
        );
        assert_eq!(report.phases[1].name, "writer_stop");
        assert_eq!(report.phases[2].name, "database_close");
        assert_eq!(report.phases[1].status, ShutdownPhaseStatus::Succeeded);
        assert_eq!(report.phases[2].status, ShutdownPhaseStatus::Succeeded);
    }
}

#[tokio::test]
async fn startup_hook_panics_still_run_cleanup_and_abort_later_startup() {
    for synchronous in [true, false] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let (later, cleanup) = (events.clone(), events.clone());
        let runtime = AsterRuntime::builder()
            .component(RuntimeServiceComponent::new(
                "service",
                RuntimeComponentKind::Core,
                async {},
                CancellationToken::default(),
                || async {},
            ))
            .component(runtime_component(
                move |registry: &mut RuntimeComponentRegistry| {
                    registry
                        .component("writer")
                        .startup(
                            "writer_start",
                            StartupPhaseFailurePolicy::Required,
                            move || {
                                assert!(!synchronous, "startup construction panicked");
                                async { panic!("startup polling panicked") }
                            },
                        )
                        .startup(
                            "later_start",
                            StartupPhaseFailurePolicy::Required,
                            move || {
                                later.lock().unwrap().push("later_start");
                                async { Ok(()) }
                            },
                        )
                        .shutdown("writer_stop", None, move || {
                            cleanup.lock().unwrap().push("writer_stop");
                            async { Ok(()) }
                        });
                },
            ))
            .build()
            .unwrap();
        let result = tokio::spawn(runtime.run())
            .await
            .expect("startup panic must become a runtime report");
        let Err(AsterRuntimeError::Startup {
            report,
            shutdown_report,
        }) = result
        else {
            panic!("startup must abort")
        };
        assert!(report.aborted());
        assert_eq!(report.phases.len(), 1);
        assert_eq!(
            report.phases[0].status,
            StartupPhaseStatus::Failed("startup phase panicked".into())
        );
        assert_eq!(
            shutdown_report.phases[0].status,
            ShutdownPhaseStatus::Succeeded
        );
        assert_eq!(*events.lock().unwrap(), ["writer_stop"]);
    }
}

#[tokio::test]
async fn optional_startup_panic_is_reported_and_later_phases_continue() {
    for synchronous in [true, false] {
        let mut coordinator = StartupCoordinator::new();
        coordinator
            .optional("optional", move || {
                assert!(!synchronous, "optional startup construction panicked");
                async { panic!("optional startup polling panicked") }
            })
            .required("next", || async { Ok(()) });
        let report = tokio::spawn(async move { coordinator.run().await })
            .await
            .expect("optional startup panic must be isolated");
        assert!(!report.aborted());
        assert_eq!(
            report.phases[0].status,
            StartupPhaseStatus::SkippedAfterFailure("startup phase panicked".into())
        );
        assert_eq!(report.phases[1].status, StartupPhaseStatus::Succeeded);
    }
}

#[tokio::test]
async fn before_shutdown_panic_still_runs_cleanup_and_retains_secondary_failure() {
    for synchronous in [true, false] {
        let runtime = AsterRuntime::builder()
            .component(RuntimeServiceComponent::new(
                "service",
                RuntimeComponentKind::Core,
                async {},
                CancellationToken::default(),
                || async {},
            ))
            .before_shutdown(move || {
                assert!(!synchronous, "before-shutdown construction panicked");
                async { panic!("before-shutdown polling panicked") }
            })
            .component(runtime_component(
                |registry: &mut RuntimeComponentRegistry| {
                    registry
                        .component("writer")
                        .shutdown("writer_stop", None, || async { Err("drain failed".into()) });
                    registry
                        .component("database")
                        .depends_on("writer")
                        .shutdown("database_close", None, || async { Ok(()) });
                },
            ))
            .build()
            .unwrap();
        let result = tokio::spawn(runtime.run())
            .await
            .expect("before-shutdown panic must not bypass component cleanup");
        let Err(AsterRuntimeError::BeforeShutdown { shutdown_report }) = result else {
            panic!("before-shutdown failure must reach caller")
        };
        assert_eq!(
            shutdown_report.phases[0].status,
            ShutdownPhaseStatus::Failed("drain failed".into())
        );
        assert_eq!(shutdown_report.phases[1].name, "database_close");
        assert_eq!(
            shutdown_report.phases[1].status,
            ShutdownPhaseStatus::Succeeded
        );
    }
}

struct SignalCapture(Arc<std::sync::atomic::AtomicUsize>);
impl Drop for SignalCapture {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[tokio::test]
async fn completed_runtime_releases_signal_callback_resources() {
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let capture = SignalCapture(dropped.clone());
    AsterRuntime::builder()
        .component(RuntimeServiceComponent::new(
            "service",
            RuntimeComponentKind::Core,
            async {},
            CancellationToken::default(),
            move || async move {
                drop(capture);
            },
        ))
        .run()
        .await
        .unwrap();
    assert_eq!(
        dropped.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "signal listener must not retain old service resources after runtime completion"
    );
}

#[tokio::test]
async fn completed_service_lifecycle_releases_signal_callback_resources() {
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let capture = SignalCapture(dropped.clone());
    ServiceLifecycle::new(async {}, CancellationToken::default())
        .run(
            move || async move {
                drop(capture);
            },
            || async {},
        )
        .await;
    assert_eq!(
        dropped.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "low-level lifecycle must also join its signal listener"
    );
}

#[tokio::test]
async fn cancelling_runtime_aborts_the_owned_signal_listener() {
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let capture = SignalCapture(dropped.clone());
    let entered = Arc::new(tokio::sync::Notify::new());
    let notify = entered.clone();
    let runtime = AsterRuntime::builder()
        .component(RuntimeServiceComponent::new(
            "service",
            RuntimeComponentKind::Core,
            async move {
                notify.notify_one();
                std::future::pending::<()>().await;
            },
            CancellationToken::default(),
            move || async move {
                drop(capture);
            },
        ))
        .build()
        .unwrap();
    let run = tokio::spawn(runtime.run());
    entered.notified().await;
    run.abort();
    assert!(run.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(1), async {
        while dropped.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelled runtime must not detach its signal listener");
    assert_eq!(dropped.load(std::sync::atomic::Ordering::SeqCst), 1);
}
