//! API response models.

use serde::Serialize;

/// Basic status response returned by the generated skeleton.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub struct StatusResponse {
    /// Cargo package name.
    pub service: &'static str,
    /// Public health or readiness status.
    pub status: &'static str,
}

/// Minimal JSON error response returned by the generated skeleton.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub struct ErrorResponse {
    /// Cargo package name.
    pub service: &'static str,
    /// Stable product error code.
    pub code: &'static str,
    /// Human-readable error summary.
    pub message: &'static str,
}
