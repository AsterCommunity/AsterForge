//! Exporter-independent typed instruments. Handles carry their instance ownership.

use crate::{MetricDescriptor, MetricKind, MetricsLimits, provider::metric_name};
use hashbrown::{Equivalent, HashMap};
use opentelemetry::{
    KeyValue,
    metrics::{
        BoundCounter, BoundGauge, BoundHistogram, BoundUpDownCounter, Counter, Gauge, Histogram,
        Meter, UpDownCounter,
    },
};
use parking_lot::Mutex;
use std::{
    hash::{Hash, Hasher},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};

/// Stable, deliberately redacted errors: never contain labels or exporter credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MetricError {
    #[error("invalid metrics configuration")]
    InvalidConfiguration,
    #[error("invalid metric descriptor")]
    InvalidDescriptor,
    #[error("duplicate metric registration")]
    DuplicateRegistration,
    #[error("metric instrument limit reached")]
    InstrumentLimit,
    #[error("metric series limit reached")]
    SeriesLimit,
    #[error("invalid metric labels")]
    InvalidLabels,
    #[error("wrong metric kind")]
    WrongKind,
    #[error("invalid metric value or counter overflow")]
    InvalidValue,
    #[error("metrics initialization failed")]
    Initialization,
    #[error("metrics exporter is unavailable")]
    Unavailable,
    #[error("metrics operation is busy")]
    Busy,
    #[error("metrics operation timed out")]
    Timeout,
    #[error("metrics export failed")]
    Export,
    #[error("metrics provider is closed")]
    Closed,
    #[error("metrics shutdown failed")]
    Shutdown,
}

pub type MetricResult<T> = std::result::Result<T, MetricError>;

/// Borrow the caller's labels for cache lookup; allocate only for new series.
struct BorrowedLabels<'a>(&'a [&'a str]);
impl Hash for BorrowedLabels<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}
impl Equivalent<Vec<String>> for BorrowedLabels<'_> {
    fn equivalent(&self, key: &Vec<String>) -> bool {
        self.0.len() == key.len() && self.0.iter().zip(key).all(|(left, right)| *left == right)
    }
}

enum Instrument {
    Counter(Counter<u64>),
    UpDownCounter(UpDownCounter<i64>),
    Snapshot,
    Gauge(Gauge<f64>),
    Histogram(Histogram<f64>),
}
enum BoundInstrument {
    Counter(BoundCounter<u64>),
    UpDownCounter(BoundUpDownCounter<i64>),
    Snapshot,
    Gauge(BoundGauge<f64>),
    Histogram(BoundHistogram<f64>),
}
struct Series {
    instrument: BoundInstrument,
    counter: AtomicU64,
    gauge: Mutex<f64>,
    sampled: AtomicBool,
}
struct HandleInner {
    descriptor: MetricDescriptor,
    instrument: Instrument,
    enabled: bool,
    closed: Arc<AtomicBool>,
    limits: MetricsLimits,
    series_count: Arc<AtomicUsize>,
    series: Arc<Mutex<HashMap<Vec<String>, Arc<Series>>>>,
    failures: Arc<AtomicU64>,
}

#[derive(Clone)]
pub struct MetricHandle {
    inner: Arc<HandleInner>,
}

impl MetricHandle {
    pub(crate) fn new(
        descriptor: MetricDescriptor,
        meter: &Meter,
        enabled: bool,
        closed: Arc<AtomicBool>,
        limits: MetricsLimits,
        series_count: Arc<AtomicUsize>,
        failures: Arc<AtomicU64>,
    ) -> Self {
        let name = metric_name(&descriptor);
        let snapshots: Arc<Mutex<HashMap<Vec<String>, Arc<Series>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let instrument = match descriptor.kind {
            MetricKind::UpDownCounter => Instrument::UpDownCounter(
                meter
                    .i64_up_down_counter(name)
                    .with_description(descriptor.help)
                    .with_unit(descriptor.unit)
                    .build(),
            ),
            MetricKind::ObservableCounter | MetricKind::ObservableUpDownCounter => {
                let state = snapshots.clone();
                let callback = move |observer: &dyn opentelemetry::metrics::AsyncInstrument<f64>| {
                    for (values, series) in state.lock().iter() {
                        let attrs: Vec<KeyValue> = descriptor
                            .labels
                            .iter()
                            .zip(values)
                            .filter(|(_, value)| !value.is_empty())
                            .map(|(key, value)| KeyValue::new(*key, value.clone()))
                            .collect();
                        let value = series.gauge.lock();
                        if series.sampled.load(Ordering::Acquire) {
                            observer.observe(*value, &attrs);
                        }
                    }
                };
                if descriptor.kind == MetricKind::ObservableCounter {
                    meter
                        .f64_observable_counter(name)
                        .with_description(descriptor.help)
                        .with_unit(descriptor.unit)
                        .with_callback(callback)
                        .build();
                } else {
                    meter
                        .f64_observable_up_down_counter(name)
                        .with_description(descriptor.help)
                        .with_unit(descriptor.unit)
                        .with_callback(callback)
                        .build();
                }
                Instrument::Snapshot
            }
            MetricKind::Counter => Instrument::Counter(
                meter
                    .u64_counter(name)
                    .with_description(descriptor.help)
                    .with_unit(descriptor.unit)
                    .build(),
            ),
            MetricKind::Gauge => Instrument::Gauge(
                meter
                    .f64_gauge(name)
                    .with_description(descriptor.help)
                    .with_unit(descriptor.unit)
                    .build(),
            ),
            MetricKind::Histogram => {
                let builder = meter
                    .f64_histogram(name)
                    .with_description(descriptor.help)
                    .with_unit(descriptor.unit);
                Instrument::Histogram(if descriptor.buckets.is_empty() {
                    builder.build()
                } else {
                    builder.with_boundaries(descriptor.buckets.to_vec()).build()
                })
            }
        };
        Self {
            inner: Arc::new(HandleInner {
                descriptor,
                instrument,
                enabled,
                closed,
                limits,
                series_count,
                series: snapshots,
                failures,
            }),
        }
    }

    #[must_use]
    pub fn kind(&self) -> MetricKind {
        self.inner.descriptor.kind
    }

    fn active(&self) -> MetricResult<bool> {
        if self.inner.closed.load(Ordering::Acquire) {
            Err(MetricError::Closed)
        } else {
            Ok(self.inner.enabled)
        }
    }

    fn series(&self, labels: &[&str]) -> MetricResult<Arc<Series>> {
        if labels.len() != self.inner.descriptor.labels.len()
            || labels
                .iter()
                .any(|label| label.len() > self.inner.limits.max_label_bytes)
        {
            return Err(MetricError::InvalidLabels);
        }
        let mut series = self.inner.series.lock();
        if let Some(value) = series.get(&BorrowedLabels(labels)) {
            return Ok(value.clone());
        }
        if series.len() >= self.inner.limits.max_series_per_instrument {
            return Err(MetricError::SeriesLimit);
        }
        self.inner
            .series_count
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                (current < self.inner.limits.max_total_series).then_some(current + 1)
            })
            .map_err(|_| MetricError::SeriesLimit)?;
        let attributes: Vec<KeyValue> = self
            .inner
            .descriptor
            .labels
            .iter()
            .zip(labels)
            .filter(|(_, value)| !value.is_empty())
            .map(|(key, value)| KeyValue::new(*key, (*value).to_owned()))
            .collect();
        let instrument = match &self.inner.instrument {
            Instrument::Counter(counter) => BoundInstrument::Counter(counter.bind(&attributes)),
            Instrument::UpDownCounter(counter) => {
                BoundInstrument::UpDownCounter(counter.bind(&attributes))
            }
            Instrument::Snapshot => BoundInstrument::Snapshot,
            Instrument::Gauge(gauge) => BoundInstrument::Gauge(gauge.bind(&attributes)),
            Instrument::Histogram(histogram) => {
                BoundInstrument::Histogram(histogram.bind(&attributes))
            }
        };
        let value = Arc::new(Series {
            instrument,
            counter: AtomicU64::new(0),
            gauge: Mutex::new(0.0),
            sampled: AtomicBool::new(false),
        });
        series.insert(
            labels.iter().map(|label| (*label).to_owned()).collect(),
            value.clone(),
        );
        Ok(value)
    }
}

#[derive(Clone)]
pub struct ProductCounter {
    handle: MetricHandle,
}
#[derive(Clone)]
pub struct ProductGauge {
    handle: MetricHandle,
}
#[derive(Clone)]
pub struct ProductHistogram {
    handle: MetricHandle,
}

macro_rules! typed {
    ($name:ident, $kind:ident) => {
        impl $name {
            /// # Errors
            /// Returns `WrongKind` if the handle is not the requested instrument kind.
            pub fn from_handle(handle: MetricHandle) -> MetricResult<Self> {
                if handle.kind() != MetricKind::$kind {
                    return Err(MetricError::WrongKind);
                }
                Ok(Self { handle })
            }
        }
    };
}
typed!(ProductCounter, Counter);
typed!(ProductGauge, Gauge);
typed!(ProductHistogram, Histogram);
typed!(ProductUpDownCounter, UpDownCounter);
typed!(ProductSnapshotCounter, ObservableCounter);
typed!(ProductSnapshotUpDownCounter, ObservableUpDownCounter);

#[derive(Clone)]
pub struct ProductUpDownCounter {
    handle: MetricHandle,
}
#[derive(Clone)]
pub struct ProductSnapshotCounter {
    handle: MetricHandle,
}
#[derive(Clone)]
pub struct ProductSnapshotUpDownCounter {
    handle: MetricHandle,
}
impl ProductUpDownCounter {
    pub fn add(&self, labels: &[&str], value: i64) {
        self.handle.diagnose(self.try_add(labels, value));
    }
    /// # Errors
    /// Rejects invalid labels, exhausted budgets and closed providers.
    pub fn try_add(&self, labels: &[&str], value: i64) -> MetricResult<()> {
        if self.handle.active()? {
            let series = self.handle.series(labels)?;
            if let BoundInstrument::UpDownCounter(counter) = &series.instrument {
                counter.add(value);
            }
        }
        Ok(())
    }
}
macro_rules! snapshot {
    ($ty:ident, $monotonic:expr) => {
        impl $ty {
            pub fn set(&self, labels: &[&str], value: f64) {
                self.handle.diagnose(self.try_set(labels, value));
            }
            /// # Errors
            /// Rejects invalid values/labels, decreasing cumulative snapshots and closed providers.
            pub fn try_set(&self, labels: &[&str], value: f64) -> MetricResult<()> {
                if !self.handle.active()? {
                    return Ok(());
                }
                if !value.is_finite() || ($monotonic && value < 0.0) {
                    return Err(MetricError::InvalidValue);
                }
                let series = self.handle.series(labels)?;
                let mut current = series.gauge.lock();
                if $monotonic && value < *current {
                    return Err(MetricError::InvalidValue);
                }
                *current = value;
                series.sampled.store(true, Ordering::Release);
                Ok(())
            }
        }
    };
}
snapshot!(ProductSnapshotCounter, true);
snapshot!(ProductSnapshotUpDownCounter, false);

impl ProductCounter {
    pub fn inc(&self, labels: &[&str], value: u64) {
        self.handle.diagnose(self.try_inc(labels, value));
    }
    /// # Errors
    /// Returns an error for closed providers, invalid labels, series limits or counter overflow.
    pub fn try_inc(&self, labels: &[&str], value: u64) -> MetricResult<()> {
        if !self.handle.active()? {
            return Ok(());
        }
        let series = self.handle.series(labels)?;
        if let BoundInstrument::Counter(counter) = &series.instrument {
            series
                .counter
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                    current.checked_add(value)
                })
                .map_err(|_| MetricError::InvalidValue)?;
            counter.add(value);
            Ok(())
        } else {
            Err(MetricError::WrongKind)
        }
    }
}

impl ProductGauge {
    pub fn set(&self, labels: &[&str], value: f64) {
        self.handle.diagnose(self.try_set(labels, value));
    }
    pub fn add(&self, labels: &[&str], value: f64) {
        self.handle.diagnose(self.try_add(labels, value));
    }
    /// # Errors
    /// Returns an error for closed providers, invalid labels, series limits or non-finite values.
    pub fn try_set(&self, labels: &[&str], value: f64) -> MetricResult<()> {
        self.update(labels, value, false)
    }
    /// # Errors
    /// Returns an error for closed providers, invalid labels, series limits or non-finite results.
    pub fn try_add(&self, labels: &[&str], value: f64) -> MetricResult<()> {
        self.update(labels, value, true)
    }
    fn update(&self, labels: &[&str], value: f64, add: bool) -> MetricResult<()> {
        if !self.handle.active()? {
            return Ok(());
        }
        if !value.is_finite() {
            return Err(MetricError::InvalidValue);
        }
        let series = self.handle.series(labels)?;
        let mut current = series.gauge.lock();
        let next = if add { *current + value } else { value };
        if !next.is_finite() {
            return Err(MetricError::InvalidValue);
        }
        if let BoundInstrument::Gauge(gauge) = &series.instrument {
            *current = next;
            // Keep the record ordered with the update; concurrent add/set must
            // not publish an older absolute value after a newer one.
            gauge.record(next);
            Ok(())
        } else {
            Err(MetricError::WrongKind)
        }
    }
}

impl ProductHistogram {
    pub fn observe(&self, labels: &[&str], value: f64) {
        self.handle.diagnose(self.try_observe(labels, value));
    }
    /// # Errors
    /// Returns an error for closed providers, invalid labels, series limits or invalid observations.
    pub fn try_observe(&self, labels: &[&str], value: f64) -> MetricResult<()> {
        if !self.handle.active()? {
            return Ok(());
        }
        if !value.is_finite() || value < 0.0 {
            return Err(MetricError::InvalidValue);
        }
        let series = self.handle.series(labels)?;
        if let BoundInstrument::Histogram(histogram) = &series.instrument {
            histogram.record(value);
            Ok(())
        } else {
            Err(MetricError::WrongKind)
        }
    }
}

impl MetricHandle {
    fn diagnose(&self, result: MetricResult<()>) {
        if let Err(error) = result {
            self.inner.failures.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(%error, "metric observation skipped");
        }
    }
}

/// Declare one typed schema and register it atomically against an explicit provider.
#[macro_export]
macro_rules! product_metrics {
    ($(#[$meta:meta])* $vis:vis struct $name:ident {
        $($(#[$field_meta:meta])* $field:ident : $kind:ident ($($args:expr),* $(,)? ) $(.with_unit($unit:expr))?),* $(,)?
    }) => {
        $(#[$meta])*
        $vis struct $name { $($(#[$field_meta])* pub $field: $crate::product_metrics!(@type $kind),)* }
        impl $name {
            /// Registers this complete schema atomically on one instance.
            ///
            /// # Errors
            /// Returns a registration error without changing the catalog when any descriptor is rejected.
            pub fn register(provider: &$crate::MetricsProvider) -> $crate::MetricResult<Self> {
                let descriptors = [$($(#[$field_meta])* $crate::MetricDescriptor::$kind($($args),*)$(.with_unit($unit))?),*];
                let mut handles = provider.register_batch(&descriptors)?.into_iter();
                Ok(Self { $($(#[$field_meta])* $field: <$crate::product_metrics!(@type $kind)>::from_handle(
                    handles.next().ok_or($crate::MetricError::InvalidDescriptor)?
                )?,)* })
            }
        }
    };
    (@type counter) => { $crate::ProductCounter };
    (@type gauge) => { $crate::ProductGauge };
    (@type up_down_counter) => { $crate::ProductUpDownCounter };
    (@type observable_counter) => { $crate::ProductSnapshotCounter };
    (@type observable_up_down_counter) => { $crate::ProductSnapshotUpDownCounter };
    (@type histogram) => { $crate::ProductHistogram };
    (@type histogram_with_buckets) => { $crate::ProductHistogram };
}
