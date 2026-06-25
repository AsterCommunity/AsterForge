//! Startup phase coordination primitives.
//!
//! This module provides the product-neutral mechanics for ordered startup phase execution:
//! duration collection, optional phase failure handling, report aggregation, and shared tracing.
//! Product crates still own concrete initialization work such as migrations, cache creation,
//! driver loading, runtime config reload, audit setup, and application state construction.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::{Duration, Instant};

type StartupFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
type StartupPhaseFn = dyn FnMut() -> StartupFuture + Send;

/// Error returned by runtime temporary directory helpers.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeTempDirError {
    /// The scope segment is empty or contains path-unsafe characters.
    #[error(
        "invalid runtime temp scope '{scope}': use non-empty ASCII letters, digits, '-' or '_'"
    )]
    InvalidScope {
        /// Invalid scope value.
        scope: String,
    },
    /// The filesystem operation failed.
    #[error("runtime temp directory IO failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Ensures the short-lived runtime temporary directory exists.
///
/// The directory is derived from [`aster_forge_utils::paths::runtime_temp_dir`], so all Aster
/// services use the same `_runtime` namespace under their configured temporary root. Products keep
/// ownership of when the directory is cleaned and how IO errors are mapped into their own error
/// types.
pub async fn ensure_runtime_temp_dir(temp_root: &str) -> std::io::Result<String> {
    let runtime_temp_dir = aster_forge_utils::paths::runtime_temp_dir(temp_root);
    tokio::fs::create_dir_all(&runtime_temp_dir).await?;
    Ok(runtime_temp_dir)
}

/// Creates a scope-local runtime temporary directory guarded by RAII cleanup.
///
/// The returned [`aster_forge_utils::raii::TempDirGuard`] removes the created directory on drop.
/// This helper is intended for one operation, such as image rendering, archive extraction, or
/// temporary external command output. It should not guard the shared `_runtime` root itself.
pub async fn create_runtime_temp_dir_guard(
    temp_root: &str,
    scope: &str,
    cleanup_label: &'static str,
) -> Result<aster_forge_utils::raii::TempDirGuard, RuntimeTempDirError> {
    validate_runtime_temp_scope(scope)?;
    let runtime_temp_dir = ensure_runtime_temp_dir(temp_root).await?;
    let scoped_root = aster_forge_utils::paths::join_path(&runtime_temp_dir, scope);
    let temp_dir = aster_forge_utils::paths::join_path(
        &scoped_root,
        &aster_forge_utils::id::new_short_token(),
    );
    tokio::fs::create_dir_all(&temp_dir).await?;
    Ok(aster_forge_utils::raii::TempDirGuard::new(
        PathBuf::from(temp_dir),
        cleanup_label,
    ))
}

fn validate_runtime_temp_scope(scope: &str) -> Result<(), RuntimeTempDirError> {
    if scope.is_empty()
        || !scope
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(RuntimeTempDirError::InvalidScope {
            scope: scope.to_string(),
        });
    }

    Ok(())
}

/// Failure policy for one startup phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupPhaseFailurePolicy {
    /// A failure aborts startup and stops later phases.
    Required,
    /// A failure is recorded and startup continues.
    Optional,
}

/// Final status for one startup phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupPhaseStatus {
    /// The phase completed successfully.
    Succeeded,
    /// A required phase failed and startup stopped.
    Failed(String),
    /// An optional phase failed and startup continued.
    SkippedAfterFailure(String),
}

impl StartupPhaseStatus {
    /// Returns whether this phase completed successfully.
    pub const fn is_success(&self) -> bool {
        matches!(self, Self::Succeeded)
    }

    /// Returns whether this phase reported an error.
    pub const fn is_failure(&self) -> bool {
        !self.is_success()
    }
}

/// Report for one executed startup phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupPhaseReport {
    /// Stable phase name.
    pub name: &'static str,
    /// Failure policy used by the phase.
    pub failure_policy: StartupPhaseFailurePolicy,
    /// Phase result.
    pub status: StartupPhaseStatus,
    /// Execution duration.
    pub duration: Duration,
}

/// Aggregate report for a startup run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupReport {
    /// Phase reports in execution order.
    pub phases: Vec<StartupPhaseReport>,
}

/// Value returned by a startup phase together with its execution report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupPhaseOutcome<T> {
    /// Value returned by the product startup phase.
    pub value: T,
    /// Report for the executed phase.
    pub report: StartupPhaseReport,
}

/// Runs one required startup phase that returns a product-owned value.
///
/// This helper is useful for startup steps that construct resources such as database handles,
/// runtime config snapshots, cache backends, driver registries, or application state. The product
/// error type is preserved while Forge still provides shared tracing and phase reporting.
pub async fn run_required_startup_phase<F, Fut, T, E>(
    name: &'static str,
    phase: F,
) -> Result<StartupPhaseOutcome<T>, E>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    tracing::info!(phase = name, "starting startup phase");
    let started_at = Instant::now();
    match phase().await {
        Ok(value) => {
            let duration = started_at.elapsed();
            tracing::info!(phase = name, ?duration, "startup phase completed");
            Ok(StartupPhaseOutcome {
                value,
                report: StartupPhaseReport {
                    name,
                    failure_policy: StartupPhaseFailurePolicy::Required,
                    status: StartupPhaseStatus::Succeeded,
                    duration,
                },
            })
        }
        Err(error) => {
            let duration = started_at.elapsed();
            tracing::error!(phase = name, ?duration, %error, "startup phase failed");
            Err(error)
        }
    }
}

/// Runs one optional startup phase and returns its report.
///
/// Optional phase failures are logged and represented as
/// [`StartupPhaseStatus::SkippedAfterFailure`], but the error does not abort startup.
pub async fn run_optional_startup_phase<F, Fut, E>(
    name: &'static str,
    phase: F,
) -> StartupPhaseReport
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), E>>,
    E: std::fmt::Display,
{
    tracing::info!(phase = name, "starting startup phase");
    let started_at = Instant::now();
    let status = match phase().await {
        Ok(()) => StartupPhaseStatus::Succeeded,
        Err(error) => StartupPhaseStatus::SkippedAfterFailure(error.to_string()),
    };
    let duration = started_at.elapsed();
    match &status {
        StartupPhaseStatus::Succeeded => {
            tracing::info!(phase = name, ?duration, "startup phase completed");
        }
        StartupPhaseStatus::SkippedAfterFailure(error) => {
            tracing::warn!(
                phase = name,
                ?duration,
                %error,
                "optional startup phase failed; continuing startup"
            );
        }
        StartupPhaseStatus::Failed(error) => {
            tracing::error!(phase = name, ?duration, %error, "startup phase failed");
        }
    }

    StartupPhaseReport {
        name,
        failure_policy: StartupPhaseFailurePolicy::Optional,
        status,
        duration,
    }
}

impl StartupReport {
    /// Returns a report from phase entries.
    pub fn new(phases: Vec<StartupPhaseReport>) -> Self {
        Self { phases }
    }

    /// Returns whether startup was aborted by a required phase failure.
    pub fn aborted(&self) -> bool {
        self.phases.iter().any(|phase| {
            matches!(
                phase.status,
                StartupPhaseStatus::Failed(_) if phase.failure_policy == StartupPhaseFailurePolicy::Required
            )
        })
    }

    /// Returns whether any phase reported an error.
    pub fn has_failures(&self) -> bool {
        self.phases.iter().any(|phase| phase.status.is_failure())
    }
}

struct RegisteredStartupPhase {
    name: &'static str,
    failure_policy: StartupPhaseFailurePolicy,
    phase: Box<StartupPhaseFn>,
}

/// Ordered startup phase coordinator.
///
/// The coordinator owns phase ordering, failure policy handling, duration collection, and tracing.
/// Product crates provide closures for the actual startup work and decide how to map report data
/// into their own diagnostics or admin surfaces.
#[derive(Default)]
pub struct StartupCoordinator {
    phases: Vec<RegisteredStartupPhase>,
}

impl StartupCoordinator {
    /// Creates an empty startup coordinator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a required startup phase.
    pub fn required<F, Fut>(&mut self, name: &'static str, phase: F) -> &mut Self
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        self.phase(name, StartupPhaseFailurePolicy::Required, phase)
    }

    /// Registers an optional startup phase.
    pub fn optional<F, Fut>(&mut self, name: &'static str, phase: F) -> &mut Self
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        self.phase(name, StartupPhaseFailurePolicy::Optional, phase)
    }

    /// Registers a startup phase with an explicit failure policy.
    pub fn phase<F, Fut>(
        &mut self,
        name: &'static str,
        failure_policy: StartupPhaseFailurePolicy,
        mut phase: F,
    ) -> &mut Self
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        self.phases.push(RegisteredStartupPhase {
            name,
            failure_policy,
            phase: Box::new(move || Box::pin(phase())),
        });
        self
    }

    /// Runs registered phases in order.
    ///
    /// Required phase failures stop later phases. Optional phase failures are logged and included in
    /// the report while later phases continue.
    pub async fn run(&mut self) -> StartupReport {
        let mut reports = Vec::with_capacity(self.phases.len());

        for phase in &mut self.phases {
            tracing::info!(phase = phase.name, "starting startup phase");
            let started_at = Instant::now();
            let result = (phase.phase)().await;
            let duration = started_at.elapsed();
            let status = match result {
                Ok(()) => StartupPhaseStatus::Succeeded,
                Err(error) if phase.failure_policy == StartupPhaseFailurePolicy::Optional => {
                    StartupPhaseStatus::SkippedAfterFailure(error)
                }
                Err(error) => StartupPhaseStatus::Failed(error),
            };

            match &status {
                StartupPhaseStatus::Succeeded => {
                    tracing::info!(phase = phase.name, ?duration, "startup phase completed");
                }
                StartupPhaseStatus::SkippedAfterFailure(error) => {
                    tracing::warn!(
                        phase = phase.name,
                        ?duration,
                        %error,
                        "optional startup phase failed; continuing startup"
                    );
                }
                StartupPhaseStatus::Failed(error) => {
                    tracing::error!(phase = phase.name, ?duration, %error, "startup phase failed");
                }
            }

            let should_abort = matches!(status, StartupPhaseStatus::Failed(_));
            reports.push(StartupPhaseReport {
                name: phase.name,
                failure_policy: phase.failure_policy,
                status,
                duration,
            });
            if should_abort {
                break;
            }
        }

        StartupReport::new(reports)
    }

    /// Returns how many phases are registered.
    pub fn len(&self) -> usize {
        self.phases.len()
    }

    /// Returns whether no phases are registered.
    pub fn is_empty(&self) -> bool {
        self.phases.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{
        RuntimeTempDirError, StartupCoordinator, StartupPhaseFailurePolicy, StartupPhaseStatus,
        create_runtime_temp_dir_guard, ensure_runtime_temp_dir, run_optional_startup_phase,
        run_required_startup_phase,
    };

    static TEMP_ID: AtomicU64 = AtomicU64::new(0);

    #[tokio::test]
    async fn ensure_runtime_temp_dir_creates_runtime_namespace() {
        let root = std::env::temp_dir().join(format!(
            "aster-forge-runtime-dirs-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let root = root.to_string_lossy().to_string();

        let runtime_dir = ensure_runtime_temp_dir(&root)
            .await
            .expect("runtime temp dir should be created");

        assert_eq!(
            runtime_dir,
            aster_forge_utils::paths::runtime_temp_dir(&root)
        );
        assert!(Path::new(&runtime_dir).is_dir());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn create_runtime_temp_dir_guard_creates_scope_local_directory_and_cleans_on_drop() {
        let root = std::env::temp_dir().join(format!(
            "aster-forge-runtime-guard-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let root = root.to_string_lossy().to_string();
        let guarded_path;

        {
            let guard = create_runtime_temp_dir_guard(&root, "thumbnail", "test runtime temp dir")
                .await
                .expect("runtime temp dir guard should be created");
            guarded_path = guard.path().to_path_buf();

            assert!(guarded_path.is_dir());
            assert!(guarded_path.starts_with(aster_forge_utils::paths::runtime_temp_dir(&root)));
            assert!(guarded_path.parent().is_some_and(|parent| {
                parent.ends_with(aster_forge_utils::paths::join_path(
                    &aster_forge_utils::paths::runtime_temp_dir(&root),
                    "thumbnail",
                ))
            }));
        }

        assert!(!guarded_path.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn create_runtime_temp_dir_guard_rejects_path_like_scope() {
        let error = match create_runtime_temp_dir_guard("target/tmp", "../bad", "test").await {
            Ok(_) => {
                panic!("path-like scope should be rejected");
            }
            Err(error) => error,
        };

        assert!(matches!(error, RuntimeTempDirError::InvalidScope { .. }));
    }

    #[tokio::test]
    async fn startup_coordinator_runs_required_phases_in_order() {
        let mut coordinator = StartupCoordinator::new();
        coordinator
            .required("database", || async { Ok(()) })
            .required("cache", || async { Ok(()) });

        let report = coordinator.run().await;

        assert_eq!(coordinator.len(), 2);
        assert!(!report.has_failures());
        assert!(!report.aborted());
        assert_eq!(report.phases[0].name, "database");
        assert_eq!(report.phases[1].name, "cache");
    }

    #[tokio::test]
    async fn startup_coordinator_aborts_after_required_failure() {
        let mut coordinator = StartupCoordinator::new();
        coordinator
            .required("database", || async {
                Err("database unavailable".to_string())
            })
            .required("cache", || async { Ok(()) });

        let report = coordinator.run().await;

        assert!(report.has_failures());
        assert!(report.aborted());
        assert_eq!(report.phases.len(), 1);
        assert_eq!(
            report.phases[0].status,
            StartupPhaseStatus::Failed("database unavailable".to_string())
        );
    }

    #[tokio::test]
    async fn startup_coordinator_continues_after_optional_failure() {
        let mut coordinator = StartupCoordinator::new();
        coordinator
            .optional("metrics", || async {
                Err("prometheus unavailable".to_string())
            })
            .required("database", || async { Ok(()) });

        let report = coordinator.run().await;

        assert!(report.has_failures());
        assert!(!report.aborted());
        assert_eq!(report.phases.len(), 2);
        assert_eq!(
            report.phases[0].status,
            StartupPhaseStatus::SkippedAfterFailure("prometheus unavailable".to_string())
        );
        assert_eq!(report.phases[1].status, StartupPhaseStatus::Succeeded);
    }

    #[tokio::test]
    async fn run_required_startup_phase_returns_product_value() {
        let outcome =
            run_required_startup_phase("build_state", || async { Ok::<_, String>("state") })
                .await
                .expect("phase should succeed");

        assert_eq!(outcome.value, "state");
        assert_eq!(outcome.report.name, "build_state");
        assert_eq!(outcome.report.status, StartupPhaseStatus::Succeeded);
        assert_eq!(
            outcome.report.failure_policy,
            StartupPhaseFailurePolicy::Required
        );
    }

    #[tokio::test]
    async fn run_required_startup_phase_preserves_product_error() {
        let error = run_required_startup_phase("build_state", || async {
            Err::<(), _>("database unavailable")
        })
        .await
        .expect_err("phase should return product error");

        assert_eq!(error, "database unavailable");
    }

    #[tokio::test]
    async fn run_optional_startup_phase_reports_failure_without_returning_error() {
        let report = run_optional_startup_phase("metrics", || async {
            Err::<(), _>("prometheus unavailable")
        })
        .await;

        assert_eq!(report.name, "metrics");
        assert_eq!(report.failure_policy, StartupPhaseFailurePolicy::Optional);
        assert_eq!(
            report.status,
            StartupPhaseStatus::SkippedAfterFailure("prometheus unavailable".to_string())
        );
    }
}
