//! Public external authentication API DTOs.

use serde::Deserialize;
#[cfg(all(debug_assertions, feature = "openapi"))]
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

#[derive(Debug, Deserialize, Validate)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct StartExternalAuthReq {
    #[validate(custom(function = "crate::api::dto::validation::validate_http_url"))]
    pub redirect_uri: String,
}

#[derive(Debug, Deserialize, Validate)]
#[cfg_attr(
    all(debug_assertions, feature = "openapi"),
    derive(IntoParams, ToSchema)
)]
pub struct ExternalAuthCallbackQuery {
    #[validate(custom(function = "crate::api::dto::validation::validate_non_blank"))]
    pub state: String,
    #[validate(custom(function = "crate::api::dto::validation::validate_non_blank"))]
    pub code: String,
}
