//! Prometheus metrics implementation.
//!
//! Forge defines recorder traits and common metric semantics. The generated service owns the
//! concrete Prometheus registry so products can add domain-specific metrics without changing Forge.

mod recorder;
mod registry;

pub use recorder::PrometheusMetricsRecorder;
pub use registry::{export_metrics, init_metrics};
