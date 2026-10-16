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
