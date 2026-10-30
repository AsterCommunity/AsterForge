//! Explicit OTLP HTTP/protobuf transport. SDK environment variables never take precedence.

use crate::{MetricError, MetricResult, MetricsProviderBuilder};
use opentelemetry_proto::tonic::collector::metrics::v1::{
    ExportMetricsServiceRequest, ExportMetricsServiceResponse,
};
use opentelemetry_sdk::{
    error::{OTelSdkError, OTelSdkResult},
    metrics::{PeriodicReader, Temporality, data::ResourceMetrics, exporter::PushMetricExporter},
};
use prost::Message;
use std::{
    collections::BTreeMap,
    fmt,
    io::Read,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Credentials and endpoint are deliberately omitted from Debug and errors.
#[derive(Clone)]
pub struct OtlpMetricsConfig {
    /// Full signal endpoint, e.g. `https://collector.example/v1/metrics`.
    pub endpoint: String,
    pub headers: BTreeMap<String, String>,
    pub timeout: Duration,
    pub interval: Duration,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
}

impl fmt::Debug for OtlpMetricsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OtlpMetricsConfig")
            .field("endpoint", &"[REDACTED]")
            .field("headers", &"[REDACTED]")
            .field("timeout", &self.timeout)
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

impl OtlpMetricsConfig {
    /// # Errors
    /// Returns a redacted error for invalid endpoints, headers, timeouts or size limits.
    pub fn validate(&self) -> MetricResult<()> {
        let url =
            reqwest::Url::parse(&self.endpoint).map_err(|_| MetricError::InvalidConfiguration)?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || self.timeout.is_zero()
            || self.timeout > Duration::from_mins(1)
            || self.interval < Duration::from_secs(1)
            || self.interval > Duration::from_hours(1)
            || self.timeout > self.interval
            || self.headers.len() > 16
            || self.max_request_bytes == 0
            || self.max_request_bytes > 64 * 1024 * 1024
            || self.max_response_bytes == 0
            || self.max_response_bytes > 1024 * 1024
        {
            return Err(MetricError::InvalidConfiguration);
        }
        for (key, value) in &self.headers {
            let name = reqwest::header::HeaderName::from_bytes(key.as_bytes())
                .map_err(|_| MetricError::InvalidConfiguration)?;
            if matches!(
                name.as_str(),
                "content-type" | "content-length" | "host" | "connection" | "transfer-encoding"
            ) || value.len() > 4096
                || reqwest::header::HeaderValue::from_str(value).is_err()
            {
                return Err(MetricError::InvalidConfiguration);
            }
        }
        Ok(())
    }
}

/// This snapshot is local and remains readable when the push destination fails.
#[derive(Clone, Default)]
pub struct OtlpExportDiagnostics {
    inner: Arc<Diagnostics>,
}
#[derive(Default)]
struct Diagnostics {
    successes: AtomicU64,
    failures: AtomicU64,
    last_success_unix_seconds: AtomicU64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OtlpExportSnapshot {
    pub successes: u64,
    pub failures: u64,
    pub last_success_unix_seconds: u64,
}
impl OtlpExportDiagnostics {
    #[must_use]
    pub fn snapshot(&self) -> OtlpExportSnapshot {
        OtlpExportSnapshot {
            successes: self.inner.successes.load(Ordering::Relaxed),
            failures: self.inner.failures.load(Ordering::Relaxed),
            last_success_unix_seconds: self.inner.last_success_unix_seconds.load(Ordering::Relaxed),
        }
    }
}

impl MetricsProviderBuilder {
    /// The standard SDK owns cumulative aggregation and its one periodic worker.
    /// One observation feeds both readers; requests never run in product call paths.
    /// # Errors
    /// Returns a redacted error if configuration is invalid or the HTTP client cannot be created.
    pub fn with_otlp(
        mut self,
        config: OtlpMetricsConfig,
    ) -> MetricResult<(Self, OtlpExportDiagnostics)> {
        config.validate()?;
        if self.otlp_diagnostics.is_some() {
            return Err(MetricError::InvalidConfiguration);
        }
        let timeout = config.timeout;
        // reqwest's blocking client must be built outside an async runtime.
        let client = std::thread::spawn(move || {
            reqwest::blocking::Client::builder()
                .timeout(timeout)
                .connect_timeout(timeout)
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build()
        })
        .join()
        .map_err(|_| MetricError::Initialization)?
        .map_err(|_| MetricError::Initialization)?;
        let interval = config.interval;
        let diagnostics = OtlpExportDiagnostics::default();
        let exporter = HttpExporter {
            config,
            client,
            diagnostics: diagnostics.clone(),
        };
        self.sdk = self.sdk.with_reader(
            PeriodicReader::builder(exporter)
                .with_interval(interval)
                .build(),
        );
        self.otlp_diagnostics = Some(diagnostics.clone());
        self.enabled = true;
        Ok((self, diagnostics))
    }
}

struct HttpExporter {
    config: OtlpMetricsConfig,
    client: reqwest::blocking::Client,
    diagnostics: OtlpExportDiagnostics,
}

impl HttpExporter {
    fn send(&self, metrics: &ResourceMetrics) -> MetricResult<()> {
        use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
        for scope in metrics.scope_metrics() {
            for metric in scope.metrics() {
                if let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data()
                    && sum
                        .data_points()
                        .any(|point| i64::try_from(point.value()).is_err())
                {
                    return Err(MetricError::InvalidValue);
                }
            }
        }
        let request: ExportMetricsServiceRequest = metrics.into();
        if request.encoded_len() > self.config.max_request_bytes {
            return Err(MetricError::Export);
        }
        let mut builder = self
            .client
            .post(&self.config.endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/x-protobuf");
        for (key, value) in &self.config.headers {
            builder = builder.header(key, value);
        }
        let response = builder
            .body(request.encode_to_vec())
            .send()
            .map_err(|_| MetricError::Export)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(MetricError::Export);
        }
        let mut bytes = Vec::new();
        response
            .take((self.config.max_response_bytes as u64).saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| MetricError::Export)?;
        if bytes.len() > self.config.max_response_bytes {
            return Err(MetricError::Export);
        }
        let response = ExportMetricsServiceResponse::decode(bytes.as_slice())
            .map_err(|_| MetricError::Export)?;
        if response.partial_success.is_some_and(|partial| {
            partial.rejected_data_points > 0 || !partial.error_message.is_empty()
        }) {
            return Err(MetricError::Export);
        }
        Ok(())
    }
}

impl PushMetricExporter for HttpExporter {
    async fn export(&self, metrics: &ResourceMetrics) -> OTelSdkResult {
        // PeriodicReader executes this on its dedicated thread. No hidden retry
        // queue: a failure is retried with the next cumulative snapshot.
        let result = self.send(metrics);
        if result.is_ok() {
            self.diagnostics
                .inner
                .successes
                .fetch_add(1, Ordering::Relaxed);
            self.diagnostics.inner.last_success_unix_seconds.store(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                Ordering::Relaxed,
            );
        } else {
            self.diagnostics
                .inner
                .failures
                .fetch_add(1, Ordering::Relaxed);
            tracing::warn!("OTLP metrics export failed");
        }
        result.map_err(|_| OTelSdkError::InternalFailure("OTLP metrics export failed".into()))
    }
    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }
    fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
        Ok(())
    }
    fn temporality(&self) -> Temporality {
        Temporality::Cumulative
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MetricDescriptor, MetricsProvider, ProductCounter};
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    fn receiver(status: &str, response: Vec<u8>) -> (String, std::thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/metrics", listener.local_addr().unwrap());
        let status = status.to_owned();
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut data = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0);
                data.extend_from_slice(&buffer[..count]);
                if let Some(offset) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&data[..offset])
                        .unwrap()
                        .to_ascii_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if data.len() >= offset + 4 + length {
                        assert!(headers.contains("authorization: static-secret"));
                        let _ = write!(
                            stream,
                            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            response.len()
                        );
                        let _ = stream.write_all(&response);
                        return data[offset + 4..offset + 4 + length].to_vec();
                    }
                }
            }
        });
        (endpoint, thread)
    }

    fn config(endpoint: String) -> OtlpMetricsConfig {
        OtlpMetricsConfig {
            endpoint,
            headers: BTreeMap::from([("authorization".into(), "static-secret".into())]),
            timeout: Duration::from_millis(250),
            interval: Duration::from_mins(1),
            max_request_bytes: 1024 * 1024,
            max_response_bytes: 1024,
        }
    }

    #[tokio::test]
    async fn http_status_attributes_are_integers_for_completed_and_failed_streams() {
        use crate::{InfrastructureMetricsRecorder, MetricsRecorder};
        use opentelemetry_proto::tonic::{common::v1::any_value::Value, metrics::v1::metric::Data};
        let (endpoint, received) = receiver("200 OK", Vec::new());
        let (builder, _) = MetricsProvider::builder()
            .with_otlp(config(endpoint))
            .unwrap();
        let provider = builder.build().unwrap();
        let recorder = InfrastructureMetricsRecorder::new(provider.clone()).unwrap();
        recorder.record_http_request("GET", "https", "/completed", 200, 0.125);
        recorder.record_http_stream_failure("GET", "https", "/failed", 206, 0.25, false);
        recorder.record_http_stream_failure("GET", "http", "", 0, 0.5, true);
        provider.force_flush(Duration::from_secs(2)).await.unwrap();
        let bytes = received.join().unwrap();
        assert_eq!(
            provider.shutdown(Duration::from_secs(2)).await,
            Err(MetricError::Shutdown)
        );
        let request = ExportMetricsServiceRequest::decode(bytes.as_slice()).unwrap();
        let metric = request
            .resource_metrics
            .iter()
            .flat_map(|r| &r.scope_metrics)
            .flat_map(|s| &s.metrics)
            .find(|m| m.name == "http.server.request.duration")
            .unwrap();
        let Some(Data::Histogram(histogram)) = &metric.data else {
            panic!("HTTP duration must be a histogram");
        };
        assert_eq!(histogram.data_points.len(), 3);
        for (route, status, error) in [
            (Some("/completed"), Some(200), None),
            (Some("/failed"), Some(206), Some("other")),
            (None, None, Some("canceled")),
        ] {
            let point = histogram
                .data_points
                .iter()
                .find(|point| {
                    point
                        .attributes
                        .iter()
                        .find(|a| a.key == "http.route")
                        .and_then(|a| a.value.as_ref())
                        .and_then(|v| v.value.as_ref())
                        == route.map(|r| Value::StringValue(r.into())).as_ref()
                })
                .unwrap();
            let attr = |key: &str| {
                point
                    .attributes
                    .iter()
                    .find(|a| a.key == key)
                    .and_then(|a| a.value.as_ref())
                    .and_then(|v| v.value.clone())
            };
            assert_eq!(point.count, 1);
            assert_eq!(
                attr("http.response.status_code"),
                status.map(Value::IntValue)
            );
            assert_eq!(
                attr("error.type"),
                error.map(|e| Value::StringValue(e.into()))
            );
            assert_eq!(
                attr("http.request.method"),
                Some(Value::StringValue("GET".into()))
            );
        }
    }

    #[tokio::test]
    async fn otlp_only_and_dual_readers_export_one_cumulative_schema() {
        for pull in [false, true] {
            let (endpoint, received) = receiver("200 OK", Vec::new());
            let builder = MetricsProvider::builder();
            #[cfg(feature = "backend-prometheus")]
            let builder = if pull {
                builder.with_prometheus().unwrap()
            } else {
                builder
            };
            #[cfg(not(feature = "backend-prometheus"))]
            let _ = pull;
            let (builder, diagnostics) = builder.with_otlp(config(endpoint)).unwrap();
            let provider = builder.build().unwrap();
            let handles = provider
                .register_batch(&[MetricDescriptor::counter(
                    "file",
                    "bytes",
                    "Bytes.",
                    &["direction"],
                )
                .with_unit("By")])
                .unwrap();
            let counter = ProductCounter::from_handle(handles[0].clone()).unwrap();
            counter.inc(&["ingress"], 5);
            counter.inc(&["ingress"], 7);
            provider.force_flush(Duration::from_secs(2)).await.unwrap();
            let request =
                ExportMetricsServiceRequest::decode(received.join().unwrap().as_slice()).unwrap();
            let metric = &request.resource_metrics[0].scope_metrics[0].metrics[0];
            assert_eq!(metric.name, "file.bytes");
            assert_eq!(metric.unit, "By");
            let Some(opentelemetry_proto::tonic::metrics::v1::metric::Data::Sum(sum)) =
                &metric.data
            else {
                panic!("expected sum");
            };
            assert_eq!(sum.aggregation_temporality, 2); // cumulative
            assert_eq!(
                sum.data_points[0].value,
                Some(opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsInt(12))
            );
            assert_eq!(sum.data_points[0].attributes[0].key, "direction");
            assert_eq!(diagnostics.snapshot().successes, 1);
            #[cfg(feature = "backend-prometheus")]
            if pull {
                let output = provider
                    .export_prometheus(Duration::from_secs(1))
                    .await
                    .unwrap();
                assert!(output.contains("file_bytes_bytes_total{direction=\"ingress\"} 12"));
                assert_eq!(
                    diagnostics.snapshot().successes,
                    1,
                    "scraping must not push"
                );
            }
            // The server accepted one request. Shutdown's last export may fail,
            // but cannot block beyond the explicit network deadline.
            assert_eq!(
                provider.shutdown(Duration::from_secs(2)).await,
                Err(MetricError::Shutdown)
            );
        }
    }

    #[tokio::test]
    async fn rejection_is_locally_observable_without_secret_details() {
        let (endpoint, received) =
            receiver("401 Unauthorized", b"secret remote diagnostics".to_vec());
        let cfg = config(endpoint);
        assert!(!format!("{cfg:?}").contains("static-secret"));
        let (builder, diagnostics) = MetricsProvider::builder().with_otlp(cfg).unwrap();
        let provider = builder.build().unwrap();
        let handle = provider
            .register_batch(&[MetricDescriptor::counter(
                "test",
                "events_total",
                "Events.",
                &[],
            )])
            .unwrap()
            .remove(0);
        ProductCounter::from_handle(handle).unwrap().inc(&[], 1);
        let _ = provider.force_flush(Duration::from_secs(2)).await;
        received.join().unwrap();
        assert_eq!(diagnostics.snapshot().failures, 1);
        assert_eq!(diagnostics.snapshot().last_success_unix_seconds, 0);
        assert_eq!(
            provider.shutdown(Duration::from_secs(2)).await,
            Err(MetricError::Shutdown)
        );
    }

    #[test]
    fn configuration_errors_are_redacted_and_bounded() {
        let mut cfg = config("https://user:secret@collector.invalid/v1/metrics".into());
        assert_eq!(cfg.validate(), Err(MetricError::InvalidConfiguration));
        assert!(!cfg.validate().unwrap_err().to_string().contains("secret"));
        cfg.endpoint = "http://127.0.0.1:4318/v1/metrics".into();
        cfg.headers.insert("content-type".into(), "bad".into());
        assert!(cfg.validate().is_err());
        cfg.headers.remove("content-type");
        cfg.timeout = Duration::ZERO;
        assert!(cfg.validate().is_err());
    }
    #[test]
    fn transport_rejects_partial_success_malformed_oversized_and_redirect_responses() {
        use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsPartialSuccess;
        let partial = ExportMetricsServiceResponse {
            partial_success: Some(ExportMetricsPartialSuccess {
                rejected_data_points: 1,
                error_message: "remote-secret".into(),
            }),
        }
        .encode_to_vec();
        for (status, response, limit) in [
            ("200 OK", partial, 1024),
            ("200 OK", vec![0xff], 1024),
            ("200 OK", vec![0; 1025], 1024),
            ("307 Temporary Redirect", Vec::new(), 1024),
        ] {
            let (endpoint, received) = receiver(status, response);
            let mut config = config(endpoint);
            config.max_response_bytes = limit;
            let exporter = HttpExporter {
                client: reqwest::blocking::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .no_proxy()
                    .timeout(config.timeout)
                    .build()
                    .unwrap(),
                config,
                diagnostics: OtlpExportDiagnostics::default(),
            };
            assert_eq!(
                exporter.send(&ResourceMetrics::default()),
                Err(MetricError::Export)
            );
            received.join().unwrap();
        }
    }

    #[test]
    fn unresponsive_collector_is_bounded_by_the_explicit_network_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut config = config(format!(
            "http://{}/v1/metrics",
            listener.local_addr().unwrap()
        ));
        config.timeout = Duration::from_millis(20);
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(100));
            drop(stream);
        });
        let exporter = HttpExporter {
            client: reqwest::blocking::Client::builder()
                .no_proxy()
                .timeout(config.timeout)
                .build()
                .unwrap(),
            config,
            diagnostics: OtlpExportDiagnostics::default(),
        };
        let start = std::time::Instant::now();
        assert_eq!(
            exporter.send(&ResourceMetrics::default()),
            Err(MetricError::Export)
        );
        assert!(start.elapsed() < Duration::from_secs(1));
        server.join().unwrap();
    }
}
