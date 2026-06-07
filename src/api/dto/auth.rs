//! Authentication API DTOs.

use serde::{Deserialize, Serialize};
#[cfg(all(debug_assertions, feature = "openapi"))]
use utoipa::ToSchema;
use validator::Validate;

#[derive(Debug, Deserialize, Validate)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct SetupReq {
    #[validate(custom(function = "crate::api::dto::validation::validate_auth_username"))]
    pub username: String,
    #[validate(custom(function = "crate::api::dto::validation::validate_auth_email"))]
    pub email: String,
    #[validate(custom(function = "crate::api::dto::validation::validate_auth_password"))]
    pub password: String,
}

#[derive(Debug, Deserialize, Validate)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct RegisterReq {
    #[validate(custom(function = "crate::api::dto::validation::validate_auth_username"))]
    pub username: String,
    #[validate(custom(function = "crate::api::dto::validation::validate_auth_email"))]
    pub email: String,
    #[validate(custom(function = "crate::api::dto::validation::validate_auth_password"))]
    pub password: String,
}

#[derive(Debug, Deserialize, Validate)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct LoginReq {
    #[validate(custom(function = "crate::api::dto::validation::validate_non_blank"))]
    pub identifier: String,
    #[validate(custom(function = "crate::api::dto::validation::validate_non_blank"))]
    pub password: String,
}

#[derive(Debug, Deserialize, Validate)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct RefreshReq {
    #[validate(custom(function = "crate::api::dto::validation::validate_non_blank"))]
    pub refresh_token: String,
}

#[derive(Debug, Deserialize, Validate)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct LogoutReq {
    #[validate(custom(function = "crate::api::dto::validation::validate_non_blank"))]
    pub refresh_token: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct CheckResp {
    pub initialized: bool,
}

#[derive(Debug, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct LogoutResp {
    pub revoked: bool,
}
