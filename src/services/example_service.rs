//! Template example service.

use crate::services::auth_service::AuthUserInfo;
use serde::Serialize;
#[cfg(all(debug_assertions, feature = "openapi"))]
use utoipa::ToSchema;

#[derive(Debug, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct ExampleMessage {
    pub message: &'static str,
    pub build_time: &'static str,
}

#[derive(Debug, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct ProtectedExampleMessage {
    pub message: &'static str,
    pub user: AuthUserInfo,
}

pub fn public_message() -> ExampleMessage {
    ExampleMessage {
        message: "AsterForge public example API is working",
        build_time: env!("ASTER_BUILD_TIME"),
    }
}

pub fn protected_message(user: AuthUserInfo) -> ProtectedExampleMessage {
    ProtectedExampleMessage {
        message: "AsterForge protected example API is working",
        user,
    }
}
