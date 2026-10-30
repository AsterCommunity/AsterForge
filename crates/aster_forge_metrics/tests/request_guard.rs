use aster_forge_metrics::*;
use std::sync::{Arc, Mutex};

#[derive(Debug, PartialEq)]
enum Event {
    Active(String, String, i64),
    Terminal(String, String, String, u16, Option<bool>),
}

#[derive(Default)]
struct Capture(Mutex<Vec<Event>>);
impl DbMetricsRecorder for Capture {
    fn enabled(&self) -> bool {
        true
    }
    fn record_db_query(&self, _: &DbQueryMetric) {}
}
impl Capture {
    fn terminal(
        &self,
        method: &str,
        scheme: &str,
        route: &str,
        status: u16,
        seconds: f64,
        failure: Option<bool>,
    ) {
        assert!(seconds.is_finite() && seconds >= 0.0);
        self.0.lock().unwrap().push(Event::Terminal(
            method.into(),
            scheme.into(),
            route.into(),
            status,
            failure,
        ));
    }
}
impl MetricsRecorder for Capture {
    fn http_active(&self, method: &str, scheme: &str, delta: i64) {
        self.0
            .lock()
            .unwrap()
            .push(Event::Active(method.into(), scheme.into(), delta));
    }
    fn record_http_request(
        &self,
        method: &str,
        scheme: &str,
        route: &str,
        status: u16,
        seconds: f64,
    ) {
        self.terminal(method, scheme, route, status, seconds, None);
    }
    fn record_http_stream_failure(
        &self,
        method: &str,
        scheme: &str,
        route: &str,
        status: u16,
        seconds: f64,
        cancelled: bool,
    ) {
        self.terminal(method, scheme, route, status, seconds, Some(cancelled));
    }
}

#[test]
fn completed_and_failed_guards_have_one_terminal_event_even_if_finished_repeatedly() {
    for failure in [false, true] {
        let recorder = Arc::new(Capture::default());
        {
            let mut guard = HttpRequestGuard::new(recorder.clone(), "POST", "https");
            guard.response("/files/{id}".into(), 201);
            if failure {
                guard.fail();
            } else {
                guard.finish();
            }
            guard.finish();
            guard.fail();
        }
        assert_eq!(
            *recorder.0.lock().unwrap(),
            vec![
                Event::Active("POST".into(), "https".into(), 1),
                Event::Active("POST".into(), "https".into(), -1),
                Event::Terminal(
                    "POST".into(),
                    "https".into(),
                    "/files/{id}".into(),
                    201,
                    failure.then_some(false)
                ),
            ]
        );
    }
}

#[test]
fn dropped_guards_preserve_known_response_and_omit_unknown_metadata() {
    for response in [None, Some(("/download/{id}", 200))] {
        let recorder = Arc::new(Capture::default());
        {
            let mut guard = HttpRequestGuard::new(recorder.clone(), "CUSTOM", "untrusted");
            if let Some((route, status)) = response {
                guard.response(route.into(), status);
            }
        }
        let (route, status) = response.unwrap_or(("", 0));
        assert_eq!(
            *recorder.0.lock().unwrap(),
            vec![
                Event::Active("_OTHER".into(), "http".into(), 1),
                Event::Active("_OTHER".into(), "http".into(), -1),
                Event::Terminal(
                    "_OTHER".into(),
                    "http".into(),
                    route.into(),
                    status,
                    Some(true)
                ),
            ]
        );
    }
}

// Recorders implementing only the terminal hook must still receive failures.
#[derive(Default)]
struct RequestOnly(Capture);
impl DbMetricsRecorder for RequestOnly {
    fn enabled(&self) -> bool {
        true
    }
    fn record_db_query(&self, _: &DbQueryMetric) {}
}
impl MetricsRecorder for RequestOnly {
    fn record_http_request(
        &self,
        method: &str,
        scheme: &str,
        route: &str,
        status: u16,
        seconds: f64,
    ) {
        self.0
            .terminal(method, scheme, route, status, seconds, None);
    }
}

#[test]
fn default_failure_hook_delegates_metadata_to_the_request_hook() {
    let recorder = Arc::new(RequestOnly::default());
    {
        let mut guard = HttpRequestGuard::new(recorder.clone(), "GET", "http");
        guard.response("/files".into(), 206);
        guard.fail();
    }
    assert_eq!(
        *recorder.0.0.lock().unwrap(),
        vec![Event::Terminal(
            "GET".into(),
            "http".into(),
            "/files".into(),
            206,
            None
        )]
    );
}

#[test]
fn noop_default_hooks_leave_no_provider_or_updater() {
    let recorder = NoopMetrics::arc();
    assert!(!recorder.enabled());
    assert!(recorder.provider().is_none());
    recorder.http_active("GET", "http", 1);
    recorder.role_readiness(true);
    recorder.db_pool("writer", 1, 0, Some(1));
    recorder.task_handler_active("cleanup", 1);
    recorder.task_handler_finished("cleanup", "returned", 0.5);
    recorder.record_http_stream_failure("GET", "http", "", 0, 0.0, true);
    drop(HttpRequestGuard::new(recorder.clone(), "GET", "http"));
    assert!(
        recorder
            .system_metrics_updater_task(tokio_util::sync::CancellationToken::new())
            .is_none()
    );
    #[cfg(feature = "runtime-health")]
    recorder.record_health(
        "ready",
        &aster_forge_runtime::SystemHealthReport::new(vec![]),
    );
}

#[test]
fn all_standard_http_methods_preserve_the_finite_vocabulary() {
    for method in [
        "GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "CONNECT", "OPTIONS", "TRACE",
    ] {
        assert_eq!(normalized_http_method(method), method);
    }
    for method in ["", "get", "CUSTOM", "_OTHER"] {
        assert_eq!(normalized_http_method(method), "_OTHER");
    }
}
