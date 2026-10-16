//! Explicit instance ownership, atomic schema registration and bounded collection.

use crate::{MetricDescriptor, MetricError, MetricHandle, MetricKind, MetricResult};
use opentelemetry::metrics::{Meter, MeterProvider};
use opentelemetry_sdk::{Resource, metrics::SdkMeterProvider};
use parking_lot::Mutex;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Semaphore;

/// Limits apply to each instance and cannot be overridden by SDK environment variables.
#[derive(Clone, Debug)]
pub struct MetricsLimits {
    pub max_instruments: usize,
    pub max_series_per_instrument: usize,
    pub max_label_bytes: usize,
    pub max_total_series: usize,
    pub max_exposition_bytes: usize,
}

impl Default for MetricsLimits {
    fn default() -> Self {
        Self {
            max_instruments: 512,
            max_series_per_instrument: 4096,
            max_label_bytes: 256,
            max_total_series: 16384,
            max_exposition_bytes: 16 * 1024 * 1024,
        }
    }
}

/// Bounded deployment identity, separate from instrument labels and product IDs.
#[derive(Clone, Debug, Default)]
pub struct MetricsResource {
    pub service_name: String,
    pub service_version: String,
    pub environment: String,
    pub role: String,
    pub instance_id: String,
}

/// Builder with independent pull and push capabilities. No global SDK provider is installed.
pub struct MetricsProviderBuilder {
    pub(crate) sdk: opentelemetry_sdk::metrics::MeterProviderBuilder,
    limits: MetricsLimits,
    pub(crate) enabled: bool,
    #[cfg(feature = "backend-prometheus")]
    registry: Option<prometheus::Registry>,
    #[cfg(feature = "otlp")]
    pub(crate) otlp_diagnostics: Option<crate::OtlpExportDiagnostics>,
}

impl MetricsProviderBuilder {
    #[must_use]
    pub fn with_limits(mut self, limits: MetricsLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Explicit resource attributes only; SDK environment resource detectors are excluded.
    /// # Errors
    /// Returns a redacted error for an empty service name or oversized/control-character attributes.
    pub fn with_resource(mut self, resource: MetricsResource) -> MetricResult<Self> {
        if resource.service_name.is_empty() {
            return Err(MetricError::InvalidConfiguration);
        }
        let mut attributes = Vec::new();
        for (key, value) in [
            ("service.name", resource.service_name),
            ("service.version", resource.service_version),
            ("deployment.environment.name", resource.environment),
            ("aster.role", resource.role),
            ("service.instance.id", resource.instance_id),
        ] {
            if value.len() > 128 || value.chars().any(char::is_control) {
                return Err(MetricError::InvalidConfiguration);
            }
            if !value.is_empty() {
                attributes.push(opentelemetry::KeyValue::new(key, value));
            }
        }
        self.sdk = self.sdk.with_resource(
            Resource::builder_empty()
                .with_attributes(attributes)
                .build(),
        );
        Ok(self)
    }

    #[cfg(feature = "backend-prometheus")]
    /// # Errors
    /// Returns an error if pull export is already configured or its reader cannot be created.
    pub fn with_prometheus(mut self) -> MetricResult<Self> {
        if self.registry.is_some() {
            return Err(MetricError::InvalidConfiguration);
        }
        let registry = prometheus::Registry::new();
        let reader = opentelemetry_prometheus::exporter()
            .with_registry(registry.clone())
            .without_units()
            .without_counter_suffixes()
            .without_target_info()
            .with_resource_selector(opentelemetry_prometheus::ResourceSelector::All)
            .scope_info_enabled(false)
            .build()
            .map_err(|_| MetricError::Initialization)?;
        self.sdk = self.sdk.with_reader(reader);
        self.registry = Some(registry);
        self.enabled = true;
        Ok(self)
    }

    /// Reader injection also permits deterministic tests and custom exporters.
    #[must_use]
    pub fn with_reader<R: opentelemetry_sdk::metrics::reader::MetricReader>(
        mut self,
        reader: R,
    ) -> Self {
        self.sdk = self.sdk.with_reader(reader);
        self.enabled = true;
        self
    }

    /// # Errors
    /// Returns an error if the resource limits are invalid.
    pub fn build(self) -> MetricResult<MetricsProvider> {
        if self.limits.max_instruments == 0
            || self.limits.max_series_per_instrument == 0
            || self.limits.max_label_bytes == 0
            || self.limits.max_total_series == 0
            || self.limits.max_total_series > 1_048_576
            || self.limits.max_exposition_bytes == 0
            || self.limits.max_instruments > 4096
            || self.limits.max_series_per_instrument > 65536
            || self.limits.max_label_bytes > 4096
            || self.limits.max_exposition_bytes > 64 * 1024 * 1024
        {
            return Err(MetricError::InvalidConfiguration);
        }
        let limit = self.limits.max_series_per_instrument;
        let sdk = self
            .sdk
            .with_view(move |_| {
                opentelemetry_sdk::metrics::Stream::builder()
                    .with_cardinality_limit(limit)
                    .build()
                    .ok()
            })
            .build();
        let meter = sdk.meter("aster_forge");
        Ok(MetricsProvider {
            inner: Arc::new(ProviderInner {
                sdk,
                meter,
                enabled: self.enabled,
                closed: Arc::new(AtomicBool::new(false)),
                limits: self.limits,
                descriptors: Mutex::new(BTreeMap::new()),
                series_count: Arc::new(AtomicUsize::new(0)),
                #[cfg(feature = "backend-prometheus")]
                registry: self.registry,
                #[cfg(feature = "otlp")]
                otlp_diagnostics: self.otlp_diagnostics,
                #[cfg(feature = "backend-prometheus")]
                gather_limit: Arc::new(Semaphore::new(2)),
                lifecycle_limit: Arc::new(Semaphore::new(1)),
                shutdown_result: Arc::new(Mutex::new(None)),
            }),
        })
    }
}

struct ProviderInner {
    sdk: SdkMeterProvider,
    meter: Meter,
    enabled: bool,
    closed: Arc<AtomicBool>,
    limits: MetricsLimits,
    descriptors: Mutex<BTreeMap<String, MetricDescriptor>>,
    series_count: Arc<AtomicUsize>,
    #[cfg(feature = "backend-prometheus")]
    registry: Option<prometheus::Registry>,
    #[cfg(feature = "otlp")]
    otlp_diagnostics: Option<crate::OtlpExportDiagnostics>,
    #[cfg(feature = "backend-prometheus")]
    gather_limit: Arc<Semaphore>,
    lifecycle_limit: Arc<Semaphore>,
    shutdown_result: Arc<Mutex<Option<MetricResult<()>>>>,
}

/// Clones share one instance; instruments always remain bound to their original instance.
#[derive(Clone)]
pub struct MetricsProvider {
    inner: Arc<ProviderInner>,
}

impl MetricsProvider {
    #[must_use]
    pub fn builder() -> MetricsProviderBuilder {
        MetricsProviderBuilder {
            sdk: SdkMeterProvider::builder().with_resource(Resource::builder_empty().build()),
            limits: MetricsLimits::default(),
            enabled: false,
            #[cfg(feature = "backend-prometheus")]
            registry: None,
            #[cfg(feature = "otlp")]
            otlp_diagnostics: None,
        }
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.inner.enabled && !self.inner.closed.load(Ordering::Acquire)
    }

    /// Returns local push-export diagnostics without invoking the exporter.
    #[cfg(feature = "otlp")]
    #[must_use]
    pub fn otlp_diagnostics(&self) -> Option<crate::OtlpExportDiagnostics> {
        self.inner.otlp_diagnostics.clone()
    }

    /// Validate the complete batch and reserve every derived exposition name before
    /// constructing SDK instruments. Failure leaves this instance unchanged.
    /// # Errors
    /// Returns an error for a closed instance, invalid descriptors, duplicate names or instrument limits. The catalog is unchanged on failure.
    pub fn register_batch(
        &self,
        descriptors: &[MetricDescriptor],
    ) -> MetricResult<Vec<MetricHandle>> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(MetricError::Closed);
        }
        let mut catalog = self.inner.descriptors.lock();
        if catalog.len().saturating_add(descriptors.len()) > self.inner.limits.max_instruments {
            return Err(MetricError::InstrumentLimit);
        }
        let mut names = BTreeSet::new();
        for descriptor in catalog.values() {
            names.extend(exported_names(descriptor));
        }
        for descriptor in descriptors {
            validate(descriptor)?;
            for name in exported_names(descriptor) {
                if !names.insert(name) {
                    return Err(MetricError::DuplicateRegistration);
                }
            }
        }
        let handles = descriptors
            .iter()
            .map(|descriptor| {
                MetricHandle::new(
                    *descriptor,
                    &self.inner.meter,
                    self.inner.enabled,
                    self.inner.closed.clone(),
                    self.inner.limits.clone(),
                    self.inner.series_count.clone(),
                )
            })
            .collect();
        for descriptor in descriptors {
            catalog.insert(metric_name(descriptor), *descriptor);
        }
        Ok(handles)
    }

    #[must_use]
    pub fn descriptors(&self) -> Vec<MetricDescriptor> {
        self.inner.descriptors.lock().values().copied().collect()
    }

    /// Prometheus export is blocking work with retained concurrency permits. A
    /// timed-out caller cannot start unlimited gathers that are still running.
    #[cfg(feature = "backend-prometheus")]
    /// # Errors
    /// Returns an error for unavailable or closed providers, concurrent overload, timeout, encoding failure or oversized output.
    pub async fn export_prometheus(&self, timeout: Duration) -> MetricResult<String> {
        use prometheus::Encoder;
        let registry = self
            .inner
            .registry
            .clone()
            .ok_or(MetricError::Unavailable)?;
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(MetricError::Closed);
        }
        let permit = self
            .inner
            .gather_limit
            .clone()
            .try_acquire_owned()
            .map_err(|_| MetricError::Busy)?;
        let max_bytes = self.inner.limits.max_exposition_bytes;
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut buffer = LimitedBuffer {
                bytes: Vec::new(),
                remaining: max_bytes,
            };
            prometheus::TextEncoder::new()
                .encode(&registry.gather(), &mut buffer)
                .map_err(|_| MetricError::Export)?;
            String::from_utf8(buffer.bytes).map_err(|_| MetricError::Export)
        });
        tokio::time::timeout(timeout, task)
            .await
            .map_err(|_| MetricError::Timeout)?
            .map_err(|_| MetricError::Export)?
    }

    /// Idempotent shutdown. If the caller times out, the job retains its permit;
    /// callers can poll the actual result with `shutdown_result`.
    /// # Errors
    /// Returns an error on concurrent lifecycle operations, timeout or SDK shutdown failure.
    pub async fn shutdown(&self, timeout: Duration) -> MetricResult<()> {
        if let Some(result) = *self.inner.shutdown_result.lock() {
            return result;
        }
        let permit = self
            .inner
            .lifecycle_limit
            .clone()
            .try_acquire_owned()
            .map_err(|_| MetricError::Busy)?;
        self.inner.closed.store(true, Ordering::Release);
        let sdk = self.inner.sdk.clone();
        let result_slot = self.inner.shutdown_result.clone();
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let result = sdk
                .shutdown_with_timeout(timeout)
                .map_err(|_| MetricError::Shutdown);
            *result_slot.lock() = Some(result);
            result
        });
        tokio::time::timeout(timeout, task)
            .await
            .map_err(|_| MetricError::Timeout)?
            .map_err(|_| MetricError::Shutdown)?
    }

    #[must_use]
    pub fn shutdown_result(&self) -> Option<MetricResult<()>> {
        *self.inner.shutdown_result.lock()
    }

    /// # Errors
    /// Returns an error for closed providers, concurrent lifecycle operations, timeout or SDK export failure.
    pub async fn force_flush(&self, timeout: Duration) -> MetricResult<()> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(MetricError::Closed);
        }
        let permit = self
            .inner
            .lifecycle_limit
            .clone()
            .try_acquire_owned()
            .map_err(|_| MetricError::Busy)?;
        let sdk = self.inner.sdk.clone();
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            sdk.force_flush().map_err(|_| MetricError::Export)
        });
        tokio::time::timeout(timeout, task)
            .await
            .map_err(|_| MetricError::Timeout)?
            .map_err(|_| MetricError::Export)?
    }
}

pub(crate) fn metric_name(descriptor: &MetricDescriptor) -> String {
    if descriptor.subsystem.is_empty() {
        descriptor.name.to_owned()
    } else {
        format!("{}_{}", descriptor.subsystem, descriptor.name)
    }
}

fn exported_names(descriptor: &MetricDescriptor) -> Vec<String> {
    let name = metric_name(descriptor);
    if descriptor.kind == MetricKind::Histogram {
        vec![
            name.clone(),
            format!("{name}_bucket"),
            format!("{name}_sum"),
            format!("{name}_count"),
        ]
    } else {
        vec![name]
    }
}

fn validate(descriptor: &MetricDescriptor) -> MetricResult<()> {
    let name = metric_name(descriptor);
    let valid_name = |value: &str| {
        value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
            && value.len() <= 255
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_')
    };
    if !valid_name(&name)
        || descriptor.help.is_empty()
        || descriptor.labels.len() > 8
        || descriptor.unit.is_empty()
        || descriptor.unit.len() > 32
        || !descriptor.unit.bytes().all(|b| b.is_ascii_graphic())
    {
        return Err(MetricError::InvalidDescriptor);
    }
    let mut labels = BTreeSet::new();
    for label in descriptor.labels {
        if !valid_name(label) || *label == "le" || !labels.insert(label) {
            return Err(MetricError::InvalidDescriptor);
        }
    }
    if (descriptor.kind != MetricKind::Histogram && !descriptor.buckets.is_empty())
        || descriptor.buckets.len() > 64
        || descriptor
            .buckets
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
        || descriptor.buckets.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(MetricError::InvalidDescriptor);
    }
    Ok(())
}

#[cfg(feature = "backend-prometheus")]
struct LimitedBuffer {
    bytes: Vec<u8>,
    remaining: usize,
}

#[cfg(feature = "backend-prometheus")]
impl std::io::Write for LimitedBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(std::io::Error::other("exposition limit"));
        }
        self.bytes.extend_from_slice(bytes);
        self.remaining -= bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(all(test, feature = "backend-prometheus"))]
mod collection_tests {
    use super::*;
    use prometheus::core::Collector;
    use std::sync::{Condvar, Mutex as StdMutex};

    #[derive(Default)]
    struct Gate {
        released: StdMutex<bool>,
        ready: Condvar,
    }
    impl Gate {
        fn release(&self) {
            *self.released.lock().unwrap() = true;
            self.ready.notify_all();
        }
    }
    struct ReleaseOnDrop(Arc<Gate>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    struct SlowCollector {
        gauge: prometheus::Gauge,
        gate: Arc<Gate>,
    }
    impl Collector for SlowCollector {
        fn desc(&self) -> Vec<&prometheus::core::Desc> {
            self.gauge.desc()
        }
        fn collect(&self) -> Vec<prometheus::proto::MetricFamily> {
            let mut released = self.gate.released.lock().unwrap();
            while !*released {
                released = self.gate.ready.wait(released).unwrap();
            }
            self.gauge.collect()
        }
    }

    #[tokio::test]
    async fn timed_out_gathers_keep_actual_work_credits_until_completion() {
        let provider = MetricsProvider::builder()
            .with_prometheus()
            .unwrap()
            .build()
            .unwrap();
        let gate = Arc::new(Gate::default());
        let _release = ReleaseOnDrop(gate.clone());
        provider
            .inner
            .registry
            .as_ref()
            .unwrap()
            .register(Box::new(SlowCollector {
                gauge: prometheus::Gauge::new("test_slow", "Slow test collector.").unwrap(),
                gate: gate.clone(),
            }))
            .unwrap();
        for _ in 0..2 {
            assert_eq!(
                provider.export_prometheus(Duration::from_millis(10)).await,
                Err(MetricError::Timeout)
            );
        }
        assert_eq!(provider.inner.gather_limit.available_permits(), 0);
        assert_eq!(
            provider.export_prometheus(Duration::from_secs(1)).await,
            Err(MetricError::Busy)
        );
        gate.release();
        tokio::time::timeout(Duration::from_secs(1), async {
            while provider.inner.gather_limit.available_permits() != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            provider
                .export_prometheus(Duration::from_secs(1))
                .await
                .unwrap()
                .contains("test_slow")
        );
        provider.shutdown(Duration::from_secs(1)).await.unwrap();
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use opentelemetry_sdk::{
        error::OTelSdkResult,
        metrics::{
            InstrumentKind, Pipeline, Temporality, data::ResourceMetrics, reader::MetricReader,
        },
    };
    use std::sync::{Condvar, Mutex as StdMutex, Weak};

    #[derive(Debug, Default)]
    struct Gate(StdMutex<bool>, Condvar);
    impl Gate {
        fn wait(&self) {
            let mut released = self.0.lock().unwrap();
            while !*released {
                released = self.1.wait(released).unwrap();
            }
        }
        fn release(&self) {
            *self.0.lock().unwrap() = true;
            self.1.notify_all();
        }
    }
    struct ReleaseOnDrop(Arc<Gate>, Arc<Gate>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.release();
            self.1.release();
        }
    }
    #[derive(Debug)]
    struct SlowReader {
        flush: Arc<Gate>,
        stop: Arc<Gate>,
    }
    impl MetricReader for SlowReader {
        fn register_pipeline(&self, _: Weak<Pipeline>) {}
        fn collect(&self, _: &mut ResourceMetrics) -> OTelSdkResult {
            Ok(())
        }
        fn force_flush(&self) -> OTelSdkResult {
            self.flush.wait();
            Ok(())
        }
        fn shutdown_with_timeout(&self, _: Duration) -> OTelSdkResult {
            self.stop.wait();
            Ok(())
        }
        fn temporality(&self, _: InstrumentKind) -> Temporality {
            Temporality::Cumulative
        }
    }
    #[tokio::test]
    async fn lifecycle_timeouts_retain_work_permit_and_report_actual_shutdown_completion() {
        let flush = Arc::new(Gate::default());
        let stop = Arc::new(Gate::default());
        let _release = ReleaseOnDrop(flush.clone(), stop.clone());
        let provider = MetricsProvider::builder()
            .with_reader(SlowReader {
                flush: flush.clone(),
                stop: stop.clone(),
            })
            .build()
            .unwrap();
        let handle = provider
            .register_batch(&[MetricDescriptor::counter("test", "events", "Events.", &[])])
            .unwrap()
            .remove(0);
        let counter = crate::ProductCounter::from_handle(handle).unwrap();
        assert_eq!(
            provider.force_flush(Duration::from_millis(10)).await,
            Err(MetricError::Timeout)
        );
        assert_eq!(
            provider.force_flush(Duration::from_secs(1)).await,
            Err(MetricError::Busy)
        );
        assert_eq!(
            provider.shutdown(Duration::from_secs(1)).await,
            Err(MetricError::Busy)
        );
        assert!(
            counter.try_inc(&[], 1).is_ok(),
            "busy shutdown must not close the provider"
        );
        flush.release();
        tokio::time::timeout(Duration::from_secs(1), async {
            while provider.inner.lifecycle_limit.available_permits() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            provider.shutdown(Duration::from_millis(10)).await,
            Err(MetricError::Timeout)
        );
        assert_eq!(provider.shutdown_result(), None);
        assert_eq!(counter.try_inc(&[], 1), Err(MetricError::Closed));
        assert_eq!(
            provider.shutdown(Duration::from_secs(1)).await,
            Err(MetricError::Busy)
        );
        stop.release();
        tokio::time::timeout(Duration::from_secs(1), async {
            while provider.shutdown_result().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(provider.shutdown_result(), Some(Ok(())));
        assert_eq!(provider.shutdown(Duration::from_secs(1)).await, Ok(()));
    }
}
