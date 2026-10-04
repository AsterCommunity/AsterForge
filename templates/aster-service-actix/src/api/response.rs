//! API response models.

use aster_forge_api::response::{ApiResponse, ApiResponseCode, ResponseEnvelopeError};
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

/// Product codes; Forge only checks the success/failure classification.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub enum ApiErrorCode {
    Success,
    EndpointNotFound,
}

impl ApiResponseCode for ApiErrorCode {
    fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }
}

// Utoipa's generic syntax uses a named type for the unit-valued data/diagnostic arguments.
type NoData = ();

/// Product error adapter: injects the code/message and retains the stable ErrorResponse schema.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub struct ErrorResponse(ApiResponse<NoData, ApiErrorCode, NoData>);

impl ErrorResponse {
    /// Maps the product's missing endpoint error into the shared wire envelope.
    ///
    /// # Errors
    /// Returns the shared state error if the product code classification is inconsistent.
    pub fn endpoint_not_found() -> Result<Self, ResponseEnvelopeError> {
        ApiResponse::error(ApiErrorCode::EndpointNotFound, "endpoint not found").map(Self)
    }
}
