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
    let builder = AsterRuntime::builder();
    let root_shutdown = builder.shutdown_token().clone();
    let service_shutdown = CancellationToken::new();
    let runtime = builder
        .component(RuntimeServiceComponent::new(
            "service",
            RuntimeComponentKind::Core,
            async move {
                notify.notify_one();
                std::future::pending::<()>().await;
            },
            service_shutdown.clone(),
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
    assert!(root_shutdown.is_cancelled());
    assert!(service_shutdown.is_cancelled());
    tokio::time::timeout(Duration::from_secs(1), async {
        while dropped.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelled runtime must not detach its signal listener");
    assert_eq!(dropped.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn service_panic_cannot_bypass_component_cleanup() {
    let cleaned = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cleanup = cleaned.clone();
    let runtime = AsterRuntime::builder()
        .component(RuntimeServiceComponent::new(
            "service",
            RuntimeComponentKind::Core,
            async {
                panic!("service driver panicked");
            },
            CancellationToken::default(),
            || async {},
        ))
        .component(runtime_component(
            move |registry: &mut RuntimeComponentRegistry| {
                registry
                    .component("database")
                    .shutdown("database_close", None, move || {
                        cleanup.store(true, std::sync::atomic::Ordering::SeqCst);
                        async { Ok(()) }
                    });
            },
        ))
        .build()
        .unwrap();
    let result = tokio::spawn(runtime.run()).await;
    assert!(
        cleaned.load(std::sync::atomic::Ordering::SeqCst),
        "service panic must run component cleanup"
    );
    let Ok(Err(AsterRuntimeError::Service {
        shutdown_report,
        before_shutdown_panicked,
    })) = result
    else {
        panic!("service panic must become a runtime failure after cleanup")
    };
    assert!(!before_shutdown_panicked);
    assert!(!shutdown_report.has_failures());
    assert_eq!(shutdown_report.phases[0].name, "database_close");
}

#[tokio::test(start_paused = true)]
async fn normal_service_exit_cancels_shared_token_before_joining_components() {
    for service_failed in [false, true] {
        let builder = AsterRuntime::builder();
        let shutdown = builder.shutdown_token().clone();
        let worker = tokio::spawn(shutdown.clone().cancelled_owned());
        let result = builder
            .component(runtime_component_with_shutdown(move |shutdown| {
                RuntimeServiceComponent::new(
                    "service",
                    RuntimeComponentKind::Core,
                    async move {
                        if service_failed {
                            Err("product service failure")
                        } else {
                            Ok(42)
                        }
                    },
                    shutdown,
                    || async {},
                )
            }))
            .component(runtime_component(
                move |registry: &mut RuntimeComponentRegistry| {
                    registry.component("worker").shutdown_once(
                        "worker_join",
                        Some(Duration::from_secs(1)),
                        worker,
                        |worker| async move { worker.await.map_err(|error| error.to_string()) },
                    );
                },
            ))
            .run()
            .await
            .unwrap();
        assert_eq!(
            result,
            if service_failed {
                Err("product service failure")
            } else {
                Ok(42)
            }
        );
        assert!(shutdown.is_cancelled());
    }
}

#[tokio::test]
async fn low_level_service_panic_still_runs_after_stop_and_preserves_panic_payload() {
    let cleaned = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cleanup = cleaned.clone();
    let result = tokio::spawn(
        ServiceLifecycle::new(
            async {
                std::panic::panic_any(42_usize);
            },
            CancellationToken::default(),
        )
        .run(
            || async {},
            move || async move {
                cleanup.store(true, std::sync::atomic::Ordering::SeqCst);
            },
        ),
    )
    .await;
    assert!(
        cleaned.load(std::sync::atomic::Ordering::SeqCst),
        "after_stop must run even when the service panics"
    );
    assert_eq!(
        result.unwrap_err().into_panic().downcast_ref::<usize>(),
        Some(&42)
    );
}

#[tokio::test]
async fn service_panic_retains_secondary_hook_and_shutdown_failures() {
    for synchronous in [true, false] {
        let builder = AsterRuntime::builder();
        let root = builder.shutdown_token().clone();
        let service_token = CancellationToken::new();
        let check_token = service_token.clone();
        let result = builder
            .component(RuntimeServiceComponent::new(
                "service",
                RuntimeComponentKind::Core,
                async { panic!("service failed") },
                service_token,
                || async {},
            ))
            .before_shutdown(move || {
                assert!(check_token.is_cancelled());
                assert!(!synchronous, "hook construction failed");
                async { panic!("hook polling failed") }
            })
            .component(runtime_component(
                |registry: &mut RuntimeComponentRegistry| {
                    registry
                        .component("writer")
                        .shutdown("drain", None, || async { Err("drain failed".into()) });
                    registry
                        .component("database")
                        .depends_on("writer")
                        .shutdown("close", None, || async { Ok(()) });
                },
            ))
            .run()
            .await;
        let Err(AsterRuntimeError::Service {
            shutdown_report,
            before_shutdown_panicked,
        }) = result
        else {
            panic!("primary service failure must retain secondary failures")
        };
        assert!(root.is_cancelled());
        assert!(before_shutdown_panicked);
        assert_eq!(
            shutdown_report.phases[0].status,
            ShutdownPhaseStatus::Failed("drain failed".into())
        );
        assert_eq!(shutdown_report.phases[1].name, "close");
        assert_eq!(
            shutdown_report.phases[1].status,
            ShutdownPhaseStatus::Succeeded
        );
    }
}

#[tokio::test(start_paused = true)]
async fn separate_service_token_cannot_replace_component_root_on_exit_or_startup_abort() {
    for startup_fails in [false, true] {
        let builder = AsterRuntime::builder();
        let root = builder.shutdown_token().clone();
        let service_token = CancellationToken::new();
        let worker = tokio::spawn(root.clone().cancelled_owned());
        let check_token = service_token.clone();
        let result = builder
            .component(RuntimeServiceComponent::new(
                "service",
                RuntimeComponentKind::Core,
                async { 42 },
                service_token.clone(),
                || async {},
            ))
            .component(runtime_component(
                move |registry: &mut RuntimeComponentRegistry| {
                    registry
                        .component("worker")
                        .startup(
                            "start",
                            StartupPhaseFailurePolicy::Required,
                            move || async move {
                                if startup_fails {
                                    Err("startup failed".into())
                                } else {
                                    Ok(())
                                }
                            },
                        )
                        .shutdown_once(
                            "join",
                            Some(Duration::from_secs(1)),
                            worker,
                            move |worker| async move {
                                assert!(check_token.is_cancelled());
                                worker.await.map_err(|error| error.to_string())
                            },
                        );
                },
            ))
            .run()
            .await;
        if startup_fails {
            let Err(AsterRuntimeError::Startup {
                shutdown_report, ..
            }) = result
            else {
                panic!("expected startup failure")
            };
            assert!(!shutdown_report.has_failures());
        } else {
            assert_eq!(result.unwrap(), 42);
        }
        assert!(root.is_cancelled());
        assert!(service_token.is_cancelled());
    }
}

#[tokio::test]
async fn cancelling_during_startup_notifies_both_token_domains() {
    let builder = AsterRuntime::builder();
    let root = builder.shutdown_token().clone();
    let service_token = CancellationToken::new();
    let entered = Arc::new(tokio::sync::Notify::new());
    let notify = entered.clone();
    let run = tokio::spawn(
        builder
            .component(RuntimeServiceComponent::new(
                "service",
                RuntimeComponentKind::Core,
                async {},
                service_token.clone(),
                || async {},
            ))
            .component(runtime_component(
                move |registry: &mut RuntimeComponentRegistry| {
                    registry.component("worker").startup(
                        "start",
                        StartupPhaseFailurePolicy::Required,
                        move || {
                            let notify = notify.clone();
                            async move {
                                notify.notify_one();
                                std::future::pending().await
                            }
                        },
                    );
                },
            ))
            .run(),
    );
    entered.notified().await;
    run.abort();
    assert!(run.await.unwrap_err().is_cancelled());
    assert!(root.is_cancelled());
    assert!(service_token.is_cancelled());
}

#[tokio::test]
async fn low_level_cleanup_panics_preserve_the_primary_payload() {
    for service_panics in [false, true] {
        for synchronous in [false, true] {
            let shutdown = CancellationToken::new();
            let check = shutdown.clone();
            let result = tokio::spawn(
                ServiceLifecycle::new(
                    async move {
                        if service_panics {
                            std::panic::panic_any(42_usize);
                        }
                        7
                    },
                    shutdown.clone(),
                )
                .run(
                    || async {},
                    move || {
                        assert!(check.is_cancelled());
                        if synchronous {
                            std::panic::panic_any(99_usize);
                        }
                        async {
                            std::panic::panic_any(99_usize);
                        }
                    },
                ),
            )
            .await;
            assert_eq!(
                result.unwrap_err().into_panic().downcast_ref::<usize>(),
                Some(&if service_panics { 42 } else { 99 })
            );
            assert!(shutdown.is_cancelled());
        }
    }
}

#[tokio::test]
async fn low_level_normal_outputs_and_cancellation_reach_cleanup_unchanged() {
    for output in [Ok(42), Err("product error")] {
        let shutdown = CancellationToken::new();
        let check = shutdown.clone();
        let actual = ServiceLifecycle::new(async move { output }, shutdown.clone())
            .run(
                || async {},
                move || async move {
                    assert!(check.is_cancelled());
                },
            )
            .await;
        assert_eq!(actual, output);
        assert!(shutdown.is_cancelled());
    }
}

#[tokio::test]
async fn cancelling_low_level_run_notifies_workers_and_releases_signal_resources() {
    let shutdown = CancellationToken::new();
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let capture = SignalCapture(dropped.clone());
    let entered = Arc::new(tokio::sync::Notify::new());
    let notify = entered.clone();
    let run = tokio::spawn(
        ServiceLifecycle::new(
            async move {
                notify.notify_one();
                std::future::pending::<()>().await;
            },
            shutdown.clone(),
        )
        .run(
            move || async move {
                drop(capture);
            },
            || async {},
        ),
    );
    entered.notified().await;
    run.abort();
    assert!(run.await.unwrap_err().is_cancelled());
    assert!(shutdown.is_cancelled());
    tokio::time::timeout(Duration::from_secs(1), async {
        while dropped.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(dropped.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn low_level_shutdown_coordinator_isolates_construction_and_polling_panics() {
    for synchronous in [true, false] {
        for timeout in [None, Some(Duration::from_secs(1))] {
            let cleaned = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let cleanup = cleaned.clone();
            let mut coordinator = ShutdownCoordinator::new();
            coordinator
                .phase("producer", timeout, move || {
                    assert!(!synchronous, "construction panicked");
                    async { panic!("polling panicked") }
                })
                .phase("database", None, move || {
                    cleanup.store(true, std::sync::atomic::Ordering::SeqCst);
                    async { Ok(()) }
                });
            let result = tokio::spawn(async move { coordinator.run().await }).await;
            assert!(
                cleaned.load(std::sync::atomic::Ordering::SeqCst),
                "one panicking phase must not skip database cleanup"
            );
            let report = result.expect("phase panic must become a report");
            assert_eq!(
                report.phases[0].status,
                ShutdownPhaseStatus::Failed("shutdown phase panicked".into())
            );
            assert_eq!(report.phases[1].status, ShutdownPhaseStatus::Succeeded);
        }
    }
}

/// Models a cancellation cleanup that fails after the phase has already been polled.
struct PanickingPhaseFuture {
    complete: bool,
    polled: bool,
}
impl std::future::Future for PanickingPhaseFuture {
    type Output = Result<(), String>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.polled = true;
        if self.complete {
            std::task::Poll::Ready(Ok(()))
        } else {
            std::task::Poll::Pending
        }
    }
}
impl Drop for PanickingPhaseFuture {
    fn drop(&mut self) {
        if self.polled {
            std::panic::panic_any(42_usize);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn phase_destructor_panic_cannot_escape_timeout_and_skip_dependency_close() {
    for (complete, timeout) in [
        (false, Some(Duration::from_secs(1))),
        (true, Some(Duration::from_secs(1))),
        (true, None),
    ] {
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cleanup = closed.clone();
        let mut registry = RuntimeComponentRegistry::new();
        registry
            .component("worker")
            .shutdown("worker_stop", timeout, move || PanickingPhaseFuture {
                complete,
                polled: false,
            });
        registry
            .component("database")
            .depends_on("worker")
            .shutdown("database_close", None, move || {
                cleanup.store(true, std::sync::atomic::Ordering::SeqCst);
                async { Ok(()) }
            });
        let result = tokio::spawn(async move { registry.shutdown().await }).await;
        assert!(
            closed.load(std::sync::atomic::Ordering::SeqCst),
            "timed-out future cleanup panic must not bypass dependent close"
        );
        let report = result.expect("phase drop panic must be isolated");
        assert_eq!(
            report.phases[0].status,
            ShutdownPhaseStatus::Failed("shutdown phase panicked".into())
        );
        assert_eq!(report.phases[1].status, ShutdownPhaseStatus::Succeeded);
    }
}

#[tokio::test]
async fn completed_service_future_destructor_panic_still_runs_component_cleanup() {
    let cleaned = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cleanup = cleaned.clone();
    let run = AsterRuntime::builder()
        .component(runtime_component_with_shutdown(|shutdown| {
            RuntimeServiceComponent::new(
                "service",
                RuntimeComponentKind::Core,
                PanickingPhaseFuture {
                    complete: true,
                    polled: false,
                },
                shutdown,
                || async {},
            )
        }))
        .component(runtime_component(
            move |registry: &mut RuntimeComponentRegistry| {
                registry
                    .component("database")
                    .shutdown("close", None, move || {
                        cleanup.store(true, std::sync::atomic::Ordering::SeqCst);
                        async { Ok(()) }
                    });
            },
        ))
        .run();
    let result = tokio::spawn(run).await;
    assert!(
        cleaned.load(std::sync::atomic::Ordering::SeqCst),
        "completed service destructor panic must not bypass cleanup"
    );
    assert!(matches!(result, Ok(Err(AsterRuntimeError::Service { .. }))));
}

#[tokio::test(start_paused = true)]
async fn low_level_shutdown_coordinator_also_isolates_phase_destructor_panics() {
    for (complete, timeout) in [
        (false, Some(Duration::from_secs(1))),
        (true, Some(Duration::from_secs(1))),
        (true, None),
    ] {
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cleanup = closed.clone();
        let mut coordinator = ShutdownCoordinator::new();
        coordinator
            .phase("worker", timeout, move || PanickingPhaseFuture {
                complete,
                polled: false,
            })
            .phase("database", None, move || {
                cleanup.store(true, std::sync::atomic::Ordering::SeqCst);
                async { Ok(()) }
            });
        let report = tokio::spawn(async move { coordinator.run().await })
            .await
            .expect("coordinator must isolate phase destruction as well as polling");
        assert!(closed.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            report.phases[0].status,
            ShutdownPhaseStatus::Failed("shutdown phase panicked".into())
        );
        assert_eq!(report.phases[1].status, ShutdownPhaseStatus::Succeeded);
    }
}

#[tokio::test]
async fn low_level_completed_service_destructor_panic_runs_cleanup_and_retains_payload() {
    let cleaned = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cleanup = cleaned.clone();
    let shutdown = CancellationToken::new();
    let result = tokio::spawn(
        ServiceLifecycle::new(
            PanickingPhaseFuture {
                complete: true,
                polled: false,
            },
            shutdown.clone(),
        )
        .run(
            || async {},
            move || async move {
                cleanup.store(true, std::sync::atomic::Ordering::SeqCst);
            },
        ),
    )
    .await;
    assert!(cleaned.load(std::sync::atomic::Ordering::SeqCst));
    assert!(shutdown.is_cancelled());
    assert_eq!(
        result.unwrap_err().into_panic().downcast_ref::<usize>(),
        Some(&42)
    );
}

#[test]
fn builder_reports_assembly_errors_and_try_components_preserve_product_errors() {
    let duplicate = AsterRuntime::builder()
        .component(RuntimeServiceComponent::new(
            "one",
            RuntimeComponentKind::Core,
            async {},
            CancellationToken::default(),
            || async {},
        ))
        .component(RuntimeServiceComponent::new(
            "two",
            RuntimeComponentKind::Core,
            async {},
            CancellationToken::default(),
            || async {},
        ))
        .build();
    assert!(matches!(
        duplicate,
        Err(AsterRuntimeError::DuplicateService)
    ));

    let missing = AsterRuntime::builder().component(try_runtime_component_with_shutdown::<
        RuntimeServiceComponent<()>,
        _,
        _,
    >(|_| {
        Err::<RuntimeServiceComponent<()>, _>("product assembly failed")
    }));
    assert!(matches!(missing, Err("product assembly failed")));

    let registry = RuntimeComponentRegistry::configured(|registry| {
        shutdown_resource_component(
            "resource",
            RuntimeComponentKind::Product,
            "resource_close",
            1_u8,
            |_| async { Ok(()) },
        )
        .register(registry);
    });
    assert_eq!(
        registry.descriptor("resource").unwrap().kind,
        RuntimeComponentKind::Product
    );
}
