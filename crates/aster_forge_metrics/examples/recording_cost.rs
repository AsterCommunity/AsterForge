//! Run with `cargo run --release -p aster_forge_metrics --features backend-prometheus --example recording_cost`.
use aster_forge_metrics::{
    MetricDescriptor, MetricResult, MetricsProvider, ProductCounter, ProductGauge, ProductHistogram,
};
use std::{hint::black_box, time::Instant};

fn measure(label: &str, mut record: impl FnMut()) {
    const ITERATIONS: u32 = 1_000_000;
    for _ in 0..1000 {
        record();
    }
    let mut best = f64::INFINITY;
    for _ in 0..3 {
        let started = Instant::now();
        for _ in 0..ITERATIONS {
            record();
        }
        best = best.min(started.elapsed().as_secs_f64() * 1e9 / f64::from(ITERATIONS));
    }
    println!("{label}: {best:.1} ns/observation (warm series, best of 3)");
}

fn main() -> MetricResult<()> {
    for enabled in [false, true] {
        let builder = MetricsProvider::builder();
        let provider = if enabled {
            builder.with_prometheus()?.build()?
        } else {
            builder.build()?
        };
        let handles = provider.register_batch(&[
            MetricDescriptor::counter("benchmark", "bytes_total", "Measured bytes.", &["mode"]),
            MetricDescriptor::gauge("benchmark", "active", "Active operations.", &["mode"]),
            MetricDescriptor::histogram(
                "benchmark",
                "duration_seconds",
                "Operation duration.",
                &["mode"],
            ),
        ])?;
        let counter = ProductCounter::from_handle(handles[0].clone())?;
        let gauge = ProductGauge::from_handle(handles[1].clone())?;
        let histogram = ProductHistogram::from_handle(handles[2].clone())?;
        let mode = if enabled { "prometheus" } else { "noop" };
        measure(&format!("{mode}/counter"), || {
            black_box(&counter).inc(black_box(&["direct"]), black_box(1));
        });
        measure(&format!("{mode}/gauge"), || {
            black_box(&gauge).set(black_box(&["direct"]), black_box(1.0));
        });
        measure(&format!("{mode}/histogram"), || {
            black_box(&histogram).observe(black_box(&["direct"]), black_box(0.01));
        });
    }
    Ok(())
}
