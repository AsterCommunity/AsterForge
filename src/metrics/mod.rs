//! Prometheus metrics implementation.

mod recorder;
mod registry;
mod system;

pub use recorder::PrometheusMetricsRecorder;
pub use registry::{
    get_metrics, init_metrics, record_application_event, record_auth_event,
    record_background_task_transition, record_db_query, record_external_operation,
    record_http_request, set_background_tasks_pending,
};
pub use system::system_metrics_updater_task;
