//! 指标记录核心接口。
//!
//! 这个模块始终编译，不依赖 Prometheus。业务代码只依赖 `MetricsRecorder`，
//! `metrics` feature 关闭时注入 `NoopMetrics`，真实 Prometheus 实现由
//! `crate::metrics` 在 feature 边界内提供。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

/// 应用指标记录接口。
///
/// 所有方法默认 no-op，方便测试和非 metrics 构建复用同一条业务路径。
#[allow(unused_variables)]
pub trait MetricsRecorder: Send + Sync {
    /// 当前 recorder 是否会真实记录指标。
    ///
    /// 用于跳过会额外产生成本的采集逻辑，例如 DB callback 和 HTTP route label。
    fn enabled(&self) -> bool {
        false
    }

    fn record_http_request(&self, method: &str, route: &str, status: u16, duration_seconds: f64) {}

    fn record_db_query(&self, info: &sea_orm::metric::Info<'_>) {}

    fn record_auth_event(&self, action: &'static str, status: &'static str, reason: &'static str) {}

    fn record_application_event(
        &self,
        category: &'static str,
        event: &'static str,
        status: &'static str,
    ) {
    }

    fn record_background_task_transition(&self, kind: &'static str, status: &'static str) {}

    fn set_background_tasks_pending(&self, pending: u64) {}

    fn record_external_operation(
        &self,
        system: &'static str,
        operation: &'static str,
        status: &'static str,
        duration_seconds: f64,
    ) {
    }

    fn system_metrics_updater_task(
        &self,
        shutdown_token: CancellationToken,
    ) -> Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        None
    }
}

pub type SharedMetricsRecorder = Arc<dyn MetricsRecorder>;

/// 非 metrics 构建和测试使用的空实现。
pub struct NoopMetrics;

impl MetricsRecorder for NoopMetrics {}

impl NoopMetrics {
    pub fn new() -> Self {
        Self
    }

    pub fn arc() -> SharedMetricsRecorder {
        Arc::new(Self::new())
    }
}

impl Default for NoopMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{MetricsRecorder, NoopMetrics};
    use sea_orm::{DatabaseBackend, Statement, metric::Info};
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn noop_metrics_reports_disabled_and_can_be_shared() {
        let recorder = NoopMetrics::new();
        assert!(!recorder.enabled());

        let default = NoopMetrics;
        assert!(!default.enabled());

        let shared = NoopMetrics::arc();
        assert!(!shared.enabled());
    }

    #[test]
    fn default_metric_methods_are_noops() {
        let recorder = NoopMetrics::new();
        let statement = Statement::from_string(DatabaseBackend::Sqlite, "SELECT 1");
        let info = Info {
            elapsed: Duration::from_millis(7),
            statement: &statement,
            failed: false,
        };

        recorder.record_http_request("GET", "/health", 200, 0.01);
        recorder.record_db_query(&info);
        recorder.record_auth_event("login", "ok", "password");
        recorder.record_application_event("config", "updated", "ok");
        recorder.record_background_task_transition("cleanup", "completed");
        recorder.set_background_tasks_pending(2);
        recorder.record_external_operation("oidc", "token", "ok", 0.02);

        assert!(
            recorder
                .system_metrics_updater_task(CancellationToken::new())
                .is_none()
        );
    }
}
