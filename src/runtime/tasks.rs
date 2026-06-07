//! Runtime background task management.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::time::Duration;

use actix_web::web;
use chrono::Utc;
use futures::FutureExt;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::config::node_mode::NodeRuntimeMode;
use crate::config::operations;
use crate::metrics_core::SharedMetricsRecorder;
use crate::runtime::{AppState, SharedRuntimeState};
use crate::services::task_service::{RuntimeTaskRunOutcome, SystemRuntimeTaskKind};

const BACKGROUND_TASK_SHUTDOWN_GRACE: Duration = Duration::from_secs(30);
const BACKGROUND_TASK_DISPATCH_ERROR_BACKOFF_CAP: Duration = Duration::from_secs(5);
const MAINTENANCE_CLEANUP_JITTER_CAP: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackgroundTaskDispatchTrigger {
    Startup,
    Timer,
    Wakeup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BackgroundTaskDispatchIteration {
    has_activity: bool,
    failed: bool,
}

impl BackgroundTaskDispatchIteration {
    fn idle() -> Self {
        Self {
            has_activity: false,
            failed: false,
        }
    }

    fn active() -> Self {
        Self {
            has_activity: true,
            failed: false,
        }
    }

    fn failed() -> Self {
        Self {
            has_activity: false,
            failed: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BackgroundTaskDispatchBackoff {
    idle_interval: Duration,
    last_error: bool,
}

impl BackgroundTaskDispatchBackoff {
    fn new(base_interval: Duration, _max_interval: Duration) -> Self {
        Self {
            idle_interval: effective_dispatch_base_interval(base_interval),
            last_error: false,
        }
    }

    fn sleep_duration(&self, base_interval: Duration, max_interval: Duration) -> Duration {
        let base_interval = effective_dispatch_base_interval(base_interval);
        let max_interval = effective_dispatch_max_interval(base_interval, max_interval);
        if self.last_error {
            return base_interval.max(BACKGROUND_TASK_DISPATCH_ERROR_BACKOFF_CAP);
        }
        self.idle_interval.max(base_interval).min(max_interval)
    }

    fn record_iteration(
        &mut self,
        trigger: BackgroundTaskDispatchTrigger,
        iteration: BackgroundTaskDispatchIteration,
        base_interval: Duration,
        max_interval: Duration,
    ) {
        let base_interval = effective_dispatch_base_interval(base_interval);
        let max_interval = effective_dispatch_max_interval(base_interval, max_interval);
        if iteration.failed {
            self.idle_interval = base_interval;
            self.last_error = true;
            return;
        }
        if iteration.has_activity || matches!(trigger, BackgroundTaskDispatchTrigger::Wakeup) {
            self.idle_interval = base_interval;
            self.last_error = false;
            return;
        }
        self.idle_interval = self
            .idle_interval
            .max(base_interval)
            .saturating_mul(2)
            .min(max_interval);
        self.last_error = false;
    }
}

pub struct BackgroundTasks {
    shutdown_token: CancellationToken,
    handles: JoinSet<()>,
}

impl BackgroundTasks {
    pub fn new() -> Self {
        Self::with_shutdown_token(CancellationToken::new())
    }

    pub fn with_shutdown_token(shutdown_token: CancellationToken) -> Self {
        Self {
            shutdown_token,
            handles: JoinSet::new(),
        }
    }

    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown_token.clone()
    }

    pub fn push<F>(&mut self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.handles.spawn(task);
    }

    pub async fn shutdown(self) {
        let BackgroundTasks {
            shutdown_token,
            mut handles,
        } = self;
        shutdown_token.cancel();

        let graceful_shutdown = async { while handles.join_next().await.is_some() {} };
        if tokio::time::timeout(BACKGROUND_TASK_SHUTDOWN_GRACE, graceful_shutdown)
            .await
            .is_err()
        {
            let aborted = handles.len();
            handles.abort_all();
            tracing::warn!(
                aborted,
                grace_secs = BACKGROUND_TASK_SHUTDOWN_GRACE.as_secs(),
                "background tasks did not stop before the shutdown grace period; aborting remaining workers"
            );
            while handles.join_next().await.is_some() {}
        }
    }
}

impl Default for BackgroundTasks {
    fn default() -> Self {
        Self::new()
    }
}

pub fn spawn_runtime_background_tasks(
    state: web::Data<AppState>,
    shutdown_token: CancellationToken,
) -> BackgroundTasks {
    match state.config.server.start_mode {
        NodeRuntimeMode::Primary => spawn_primary_background_tasks(state, shutdown_token),
        NodeRuntimeMode::Follower => spawn_follower_background_tasks(state, shutdown_token),
    }
}

pub fn spawn_follower_background_tasks(
    state: web::Data<AppState>,
    shutdown_token: CancellationToken,
) -> BackgroundTasks {
    tracing::info!("starting follower runtime background tasks");
    build_background_tasks_base(&state.metrics, shutdown_token)
}

pub fn spawn_primary_background_tasks(
    state: web::Data<AppState>,
    shutdown_token: CancellationToken,
) -> BackgroundTasks {
    tracing::info!("starting primary runtime background tasks");
    let mut tasks = build_background_tasks_base(&state.metrics, shutdown_token);
    let shutdown_token = tasks.shutdown_token();

    tasks.push(spawn_background_task_dispatcher(
        shutdown_token.clone(),
        state.clone(),
    ));

    tasks.push(spawn_periodic(
        SystemRuntimeTaskKind::MailOutboxDispatch,
        mail_outbox_dispatch_interval,
        None,
        shutdown_token.clone(),
        state.clone(),
        |s| async move {
            match crate::services::mail_outbox_service::dispatch_due(s.get_ref()).await {
                Ok(stats)
                    if stats.claimed > 0
                        || stats.sent > 0
                        || stats.retried > 0
                        || stats.failed > 0 =>
                {
                    tracing::info!(
                        claimed = stats.claimed,
                        sent = stats.sent,
                        retried = stats.retried,
                        failed = stats.failed,
                        "processed mail outbox batch"
                    );
                    RuntimeTaskRunOutcome::succeeded(Some(format!(
                        "claimed {}, sent {}, retried {}, failed {}",
                        stats.claimed, stats.sent, stats.retried, stats.failed
                    )))
                }
                Ok(_) => RuntimeTaskRunOutcome::quiet(),
                Err(error) => {
                    tracing::warn!("mail outbox dispatch failed: {error}");
                    RuntimeTaskRunOutcome::failed(
                        Some("Mail outbox dispatch failed".to_string()),
                        error.to_string(),
                    )
                }
            }
        },
    ));

    tasks.push(spawn_periodic(
        SystemRuntimeTaskKind::SystemHealthCheck,
        maintenance_cleanup_interval,
        None,
        shutdown_token.clone(),
        state.clone(),
        |s| async move {
            let report = crate::services::health_service::run_system_health_checks(s.get_ref()).await;
            let status = match report.status() {
                crate::services::health_service::HealthStatus::Healthy => {
                    crate::services::task_service::types::RuntimeSystemHealthStatus::Healthy
                }
                crate::services::health_service::HealthStatus::Degraded => {
                    crate::services::task_service::types::RuntimeSystemHealthStatus::Degraded
                }
                crate::services::health_service::HealthStatus::Unhealthy => {
                    crate::services::task_service::types::RuntimeSystemHealthStatus::Unhealthy
                }
            };
            let system_health =
                crate::services::task_service::types::RuntimeSystemHealthResult {
                    status,
                    components: report
                        .components
                        .into_iter()
                        .map(|component| {
                            let status = match component.status {
                                crate::services::health_service::HealthStatus::Healthy => {
                                    crate::services::task_service::types::RuntimeSystemHealthStatus::Healthy
                                }
                                crate::services::health_service::HealthStatus::Degraded => {
                                    crate::services::task_service::types::RuntimeSystemHealthStatus::Degraded
                                }
                                crate::services::health_service::HealthStatus::Unhealthy => {
                                    crate::services::task_service::types::RuntimeSystemHealthStatus::Unhealthy
                                }
                            };
                            crate::services::task_service::types::RuntimeSystemHealthComponent {
                                name: component.name.to_string(),
                                status,
                                message: component.message,
                            }
                        })
                        .collect(),
                };
            RuntimeTaskRunOutcome::succeeded_with_system_health(
                Some("System health check completed".to_string()),
                system_health,
            )
        },
    ));

    tasks.push(spawn_periodic(
        SystemRuntimeTaskKind::AuthSessionCleanup,
        maintenance_cleanup_interval,
        Some(MAINTENANCE_CLEANUP_JITTER_CAP),
        shutdown_token.clone(),
        state.clone(),
        |s| async move {
            match crate::services::auth_service::cleanup_expired_auth_sessions(s.get_ref()).await {
                Ok(count) if count > 0 => {
                    tracing::info!(count, "cleaned up expired auth sessions");
                    RuntimeTaskRunOutcome::succeeded(Some(format!(
                        "cleaned up {count} expired auth sessions"
                    )))
                }
                Ok(_) => RuntimeTaskRunOutcome::quiet(),
                Err(error) => RuntimeTaskRunOutcome::failed(
                    Some("Auth session cleanup failed".to_string()),
                    error.to_string(),
                ),
            }
        },
    ));

    tasks.push(spawn_periodic(
        SystemRuntimeTaskKind::ExternalAuthFlowCleanup,
        maintenance_cleanup_interval,
        Some(MAINTENANCE_CLEANUP_JITTER_CAP),
        shutdown_token.clone(),
        state.clone(),
        |s| async move {
            match crate::services::external_auth_service::cleanup_expired_flows(s.get_ref()).await {
                Ok(count) if count > 0 => {
                    tracing::info!(count, "cleaned up expired external auth flows");
                    RuntimeTaskRunOutcome::succeeded(Some(format!(
                        "cleaned up {count} expired external auth flows"
                    )))
                }
                Ok(_) => RuntimeTaskRunOutcome::quiet(),
                Err(error) => RuntimeTaskRunOutcome::failed(
                    Some("External auth flow cleanup failed".to_string()),
                    error.to_string(),
                ),
            }
        },
    ));

    tasks.push(spawn_periodic(
        SystemRuntimeTaskKind::AuditCleanup,
        maintenance_cleanup_interval,
        Some(MAINTENANCE_CLEANUP_JITTER_CAP),
        shutdown_token.clone(),
        state.clone(),
        |s| async move {
            match crate::services::audit_service::cleanup_expired(s.get_ref()).await {
                Ok(count) if count > 0 => {
                    tracing::info!(count, "cleaned up expired audit logs");
                    RuntimeTaskRunOutcome::succeeded(Some(format!(
                        "cleaned up {count} expired audit logs"
                    )))
                }
                Ok(_) => RuntimeTaskRunOutcome::quiet(),
                Err(error) => RuntimeTaskRunOutcome::failed(
                    Some("Audit cleanup failed".to_string()),
                    error.to_string(),
                ),
            }
        },
    ));

    tasks.push(spawn_periodic(
        SystemRuntimeTaskKind::TaskCleanup,
        maintenance_cleanup_interval,
        Some(MAINTENANCE_CLEANUP_JITTER_CAP),
        shutdown_token,
        state,
        |s| async move {
            match crate::services::task_service::cleanup_expired(s.get_ref()).await {
                Ok(count) if count > 0 => {
                    tracing::info!(count, "cleaned up expired task temp dirs");
                    RuntimeTaskRunOutcome::succeeded(Some(format!(
                        "cleaned up {count} expired task temp dirs"
                    )))
                }
                Ok(_) => RuntimeTaskRunOutcome::quiet(),
                Err(error) => RuntimeTaskRunOutcome::failed(
                    Some("Task cleanup failed".to_string()),
                    error.to_string(),
                ),
            }
        },
    ));

    tasks
}

async fn spawn_periodic<F, I, Fut>(
    name: SystemRuntimeTaskKind,
    interval_fn: I,
    jitter_cap: Option<Duration>,
    shutdown_token: CancellationToken,
    state: web::Data<AppState>,
    task_fn: F,
) where
    I: Fn(&AppState) -> Duration + Send + Sync + 'static,
    F: Fn(web::Data<AppState>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = RuntimeTaskRunOutcome> + Send + 'static,
{
    let task_name = name.as_str();
    if shutdown_token.is_cancelled() {
        return;
    }
    run_periodic_iteration(name, &state, &task_fn)
        .instrument(tracing::info_span!("bg_task", task.name = task_name))
        .await;

    loop {
        let sleep_duration = periodic_sleep_duration(interval_fn(state.get_ref()), jitter_cap);
        tokio::select! {
            biased;
            _ = shutdown_token.cancelled() => break,
            _ = tokio::time::sleep(sleep_duration) => {}
        }

        if shutdown_token.is_cancelled() {
            break;
        }

        run_periodic_iteration(name, &state, &task_fn)
            .instrument(tracing::info_span!("bg_task", task.name = task_name))
            .await;
    }
}

async fn run_periodic_iteration<F, Fut>(
    name: SystemRuntimeTaskKind,
    state: &web::Data<AppState>,
    task_fn: &F,
) where
    F: Fn(web::Data<AppState>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = RuntimeTaskRunOutcome> + Send + 'static,
{
    let task_name = name.as_str();
    let started_at = Utc::now();
    let outcome = match AssertUnwindSafe(task_fn(state.clone()))
        .catch_unwind()
        .await
    {
        Ok(outcome) => outcome,
        Err(panic) => {
            let panic_message = panic_payload_message(&panic);
            tracing::error!("background task '{task_name}' panicked: {panic_message}");
            RuntimeTaskRunOutcome::failed(Some("Task panicked".to_string()), panic_message)
        }
    };
    let finished_at = Utc::now();

    if let Err(error) = crate::services::task_service::record_runtime_task_run(
        state.get_ref(),
        name,
        started_at,
        finished_at,
        &outcome,
    )
    .await
    {
        tracing::warn!("failed to record runtime task '{task_name}': {error}");
    }
}

async fn spawn_background_task_dispatcher(
    shutdown_token: CancellationToken,
    state: web::Data<AppState>,
) {
    let mut backoff = BackgroundTaskDispatchBackoff::new(
        background_task_dispatch_interval(state.get_ref()),
        background_task_dispatch_idle_max_interval(state.get_ref()),
    );
    if shutdown_token.is_cancelled() {
        return;
    }
    let iteration = run_background_task_dispatch_iteration(&state, shutdown_token.clone())
        .instrument(tracing::info_span!(
            "bg_task",
            task.name = SystemRuntimeTaskKind::BackgroundTaskDispatch.as_str()
        ))
        .await;
    backoff.record_iteration(
        BackgroundTaskDispatchTrigger::Startup,
        iteration,
        background_task_dispatch_interval(state.get_ref()),
        background_task_dispatch_idle_max_interval(state.get_ref()),
    );

    loop {
        let sleep_duration = backoff.sleep_duration(
            background_task_dispatch_interval(state.get_ref()),
            background_task_dispatch_idle_max_interval(state.get_ref()),
        );
        let trigger = tokio::select! {
            biased;
            _ = shutdown_token.cancelled() => break,
            _ = state.background_task_dispatch_wakeup.notified() => {
                BackgroundTaskDispatchTrigger::Wakeup
            }
            _ = tokio::time::sleep(sleep_duration) => {
                BackgroundTaskDispatchTrigger::Timer
            }
        };

        if shutdown_token.is_cancelled() {
            break;
        }

        let iteration = run_background_task_dispatch_iteration(&state, shutdown_token.clone())
            .instrument(tracing::info_span!(
                "bg_task",
                task.name = SystemRuntimeTaskKind::BackgroundTaskDispatch.as_str()
            ))
            .await;
        backoff.record_iteration(
            trigger,
            iteration,
            background_task_dispatch_interval(state.get_ref()),
            background_task_dispatch_idle_max_interval(state.get_ref()),
        );
    }
}

async fn run_background_task_dispatch_iteration(
    state: &web::Data<AppState>,
    shutdown_token: CancellationToken,
) -> BackgroundTaskDispatchIteration {
    let started_at = Utc::now();
    let (iteration, outcome) = match AssertUnwindSafe(
        crate::services::task_service::dispatch::dispatch_due_with_shutdown(
            state.get_ref(),
            shutdown_token,
        ),
    )
    .catch_unwind()
    .await
    {
        Ok(result) => {
            let iteration = match &result {
                Ok(stats) if stats.has_activity() => BackgroundTaskDispatchIteration::active(),
                Ok(_) => BackgroundTaskDispatchIteration::idle(),
                Err(_) => BackgroundTaskDispatchIteration::failed(),
            };
            (iteration, background_task_dispatch_outcome(result))
        }
        Err(panic) => {
            let panic_message = panic_payload_message(&panic);
            tracing::error!("background task 'background-task-dispatch' panicked: {panic_message}");
            (
                BackgroundTaskDispatchIteration::failed(),
                RuntimeTaskRunOutcome::failed(Some("Task panicked".to_string()), panic_message),
            )
        }
    };
    let finished_at = Utc::now();

    if let Err(error) = crate::services::task_service::record_runtime_task_run(
        state.get_ref(),
        SystemRuntimeTaskKind::BackgroundTaskDispatch,
        started_at,
        finished_at,
        &outcome,
    )
    .await
    {
        tracing::warn!("failed to record runtime task 'background-task-dispatch': {error}");
    }

    iteration
}

fn background_task_dispatch_outcome(
    result: crate::errors::Result<crate::services::task_service::DispatchStats>,
) -> RuntimeTaskRunOutcome {
    match result {
        Ok(stats) => {
            if stats.has_activity() {
                tracing::info!(
                    claimed = stats.claimed,
                    succeeded = stats.succeeded,
                    retried = stats.retried,
                    failed = stats.failed,
                    "processed background task batch"
                );
            }
            RuntimeTaskRunOutcome::quiet()
        }
        Err(error) => {
            tracing::warn!("background task dispatch failed: {error}");
            RuntimeTaskRunOutcome::failed(
                Some("Background task dispatch failed".to_string()),
                error.to_string(),
            )
        }
    }
}

fn periodic_sleep_duration(base_interval: Duration, jitter_cap: Option<Duration>) -> Duration {
    use rand::RngExt;

    let Some(jitter_cap) = jitter_cap else {
        return base_interval;
    };
    let max_jitter_ms = effective_jitter_cap(base_interval, jitter_cap).as_millis();
    if max_jitter_ms == 0 {
        return base_interval;
    }

    let max_jitter_ms =
        crate::utils::numbers::u128_to_u64(max_jitter_ms.min(u128::from(u64::MAX)), "jitter")
            .unwrap_or(u64::MAX);
    let mut rng = rand::rng();
    let jitter_ms = rng.random_range(0..=max_jitter_ms);
    base_interval.saturating_add(Duration::from_millis(jitter_ms))
}

fn effective_jitter_cap(base_interval: Duration, jitter_cap: Duration) -> Duration {
    let bounded_ms = crate::utils::numbers::u128_to_u64(
        base_interval.as_millis().min(u128::from(u64::MAX)),
        "base interval millis",
    )
    .unwrap_or(u64::MAX)
        / 10;
    jitter_cap.min(Duration::from_millis(bounded_ms))
}

fn effective_dispatch_base_interval(base_interval: Duration) -> Duration {
    if base_interval.is_zero() {
        Duration::from_secs(1)
    } else {
        base_interval
    }
}

fn effective_dispatch_max_interval(base_interval: Duration, max_interval: Duration) -> Duration {
    max_interval.max(effective_dispatch_base_interval(base_interval))
}

fn build_background_tasks_base(
    metrics: &SharedMetricsRecorder,
    shutdown_token: CancellationToken,
) -> BackgroundTasks {
    let mut tasks = BackgroundTasks::with_shutdown_token(shutdown_token);
    if let Some(task) = metrics.system_metrics_updater_task(tasks.shutdown_token()) {
        tasks.push(task);
    }
    tasks
}

fn background_task_dispatch_interval(state: &impl SharedRuntimeState) -> Duration {
    Duration::from_secs(operations::background_task_dispatch_interval_secs(
        state.runtime_config(),
    ))
}

fn background_task_dispatch_idle_max_interval(state: &impl SharedRuntimeState) -> Duration {
    Duration::from_secs(operations::background_task_dispatch_idle_max_interval_secs(
        state.runtime_config(),
    ))
}

fn maintenance_cleanup_interval(state: &impl SharedRuntimeState) -> Duration {
    Duration::from_secs(operations::maintenance_cleanup_interval_secs(
        state.runtime_config(),
    ))
}

fn mail_outbox_dispatch_interval(state: &impl SharedRuntimeState) -> Duration {
    Duration::from_secs(operations::mail_outbox_dispatch_interval_secs(
        state.runtime_config(),
    ))
}

fn panic_payload_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_only_awaits_each_handle_once() {
        let mut tasks = BackgroundTasks::new();
        tasks.push(async {});

        tasks.shutdown().await;
    }

    #[tokio::test]
    async fn external_shutdown_token_stops_background_worker_before_shutdown_join() {
        let shutdown_token = CancellationToken::new();
        let mut tasks = BackgroundTasks::with_shutdown_token(shutdown_token.clone());
        let (stopped_tx, stopped_rx) = tokio::sync::oneshot::channel();

        tasks.push({
            let shutdown_token = shutdown_token.clone();
            async move {
                shutdown_token.cancelled().await;
                let _ = stopped_tx.send(());
            }
        });

        shutdown_token.cancel();
        tokio::time::timeout(Duration::from_millis(50), stopped_rx)
            .await
            .expect("background worker should observe external shutdown")
            .expect("background worker should report shutdown");

        tasks.shutdown().await;
    }
}
