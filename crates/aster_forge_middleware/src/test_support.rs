//! Recorder fixture shared by transport tests.
use aster_forge_metrics::{MetricsRecorder, SharedMetricsRecorder};
use std::sync::{Arc, Mutex};
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HttpMetricRecord {
    pub(crate) method: String,
    pub(crate) route: String,
    pub(crate) status: u16,
    pub(crate) duration_seconds: f64,
}

pub(crate) struct RecordingMetrics {
    enabled: bool,
    records: Mutex<Vec<HttpMetricRecord>>,
}

impl RecordingMetrics {
    pub(crate) fn enabled() -> Arc<Self> {
        Arc::new(Self {
            enabled: true,
            records: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn disabled() -> Arc<Self> {
        Arc::new(Self {
            enabled: false,
            records: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn shared(self: &Arc<Self>) -> SharedMetricsRecorder {
        self.clone()
    }

    pub(crate) fn records(&self) -> Vec<HttpMetricRecord> {
        self.records.lock().expect("metrics records lock").clone()
    }
}

impl aster_forge_metrics::DbMetricsRecorder for RecordingMetrics {
    fn enabled(&self) -> bool {
        self.enabled
    }

    fn record_db_query(&self, _metric: &aster_forge_metrics::DbQueryMetric) {}
}

impl MetricsRecorder for RecordingMetrics {
    fn record_http_request(&self, method: &str, route: &str, status: u16, duration_seconds: f64) {
        self.records
            .lock()
            .expect("metrics records lock")
            .push(HttpMetricRecord {
                method: method.to_string(),
                route: route.to_string(),
                status,
                duration_seconds,
            });
    }
}
