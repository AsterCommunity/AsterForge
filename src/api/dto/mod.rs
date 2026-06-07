//! API data transfer objects.
//!
//! Route handlers should import request/response contracts from this module
//! instead of defining public API structs inline.

pub mod admin;
pub mod auth;
pub mod external_auth;
pub(crate) mod validation;

pub use admin::{
    AdminAuditLogSortQuery, AdminTaskCleanupReq, AdminTaskListQuery, CreateExternalAuthProviderReq,
    ExternalAuthProviderTestParamsReq, RemovedCountResponse, SetConfigReq,
    UpdateExternalAuthProviderReq,
};
pub use auth::{CheckResp, LoginReq, LogoutReq, LogoutResp, RefreshReq, RegisterReq, SetupReq};
pub use external_auth::{ExternalAuthCallbackQuery, StartExternalAuthReq};
pub(crate) use validation::validate_request;
