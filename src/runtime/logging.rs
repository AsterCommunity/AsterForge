//! Tracing subscriber setup.

use crate::config::LoggingConfig;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

pub struct LoggingInitResult {
    pub guard: Option<WorkerGuard>,
    pub warning: Option<String>,
}

pub fn init_logging(config: &LoggingConfig) -> LoggingInitResult {
    let filter = EnvFilter::try_new(&config.level).unwrap_or_else(|error| {
        eprintln!(
            "invalid log level '{}': {error}; falling back to info",
            config.level
        );
        EnvFilter::new("info")
    });

    let registry = tracing_subscriber::registry().with(filter);
    match config.format.as_str() {
        "json" => registry.with(fmt::layer().json()).init(),
        _ => registry.with(fmt::layer().compact()).init(),
    }

    LoggingInitResult {
        guard: None,
        warning: None,
    }
}
