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
    stream_failures: Mutex<Vec<bool>>,
    active_requests: Mutex<i64>,
}

impl RecordingMetrics {
    pub(crate) fn enabled() -> Arc<Self> {
        Arc::new(Self {
            enabled: true,
            records: Mutex::new(Vec::new()),
            stream_failures: Mutex::new(Vec::new()),
            active_requests: Mutex::new(0),
        })
    }

    pub(crate) fn disabled() -> Arc<Self> {
        Arc::new(Self {
            enabled: false,
            records: Mutex::new(Vec::new()),
            stream_failures: Mutex::new(Vec::new()),
            active_requests: Mutex::new(0),
        })
    }

    pub(crate) fn shared(self: &Arc<Self>) -> SharedMetricsRecorder {
        self.clone()
    }

    pub(crate) fn records(&self) -> Vec<HttpMetricRecord> {
        self.records.lock().expect("metrics records lock").clone()
    }

    pub(crate) fn stream_failures(&self) -> Vec<bool> {
        self.stream_failures
            .lock()
            .expect("metrics failures lock")
            .clone()
    }

    pub(crate) fn active_requests(&self) -> i64 {
        *self.active_requests.lock().expect("metrics active lock")
    }
}

impl aster_forge_metrics::DbMetricsRecorder for RecordingMetrics {
    fn enabled(&self) -> bool {
        self.enabled
    }

    fn record_db_query(&self, _metric: &aster_forge_metrics::DbQueryMetric) {}
}

impl MetricsRecorder for RecordingMetrics {
    fn http_active(&self, _: &str, _: &str, delta: i64) {
        *self.active_requests.lock().expect("metrics active lock") += delta;
    }

    fn record_http_stream_failure(
        &self,
        method: &str,
        scheme: &str,
        route: &str,
        status: u16,
        duration_seconds: f64,
        cancelled: bool,
    ) {
        self.stream_failures
            .lock()
            .expect("metrics failures lock")
            .push(cancelled);
        self.record_http_request(method, scheme, route, status, duration_seconds);
    }

    fn record_http_request(
        &self,
        method: &str,
        _scheme: &str,
        route: &str,
        status: u16,
        duration_seconds: f64,
    ) {
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
