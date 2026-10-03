//! Axum adapters installed with `from_fn` / `from_fn_with_state`.
pub mod client_ip;
pub mod cors;
pub mod csrf;
#[cfg(feature = "metrics")]
pub mod metrics;
pub mod rate_limit;
pub mod request_id;
pub mod security_headers;

#[cfg(feature = "metrics")]
pub use metrics::metrics;
pub use request_id::request_id;
pub use security_headers::security_headers;
