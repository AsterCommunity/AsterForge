//! Axum compatibility adapters.
#[cfg(feature = "metrics")]
pub mod metrics;
pub mod request_id;
pub mod security_headers;

#[cfg(feature = "metrics")]
pub use metrics::metrics;
pub use request_id::request_id;
pub use security_headers::security_headers;
