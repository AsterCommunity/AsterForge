//! Admin API DTOs.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(all(debug_assertions, feature = "openapi"))]
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use crate::api::pagination::{AdminAuditLogSortBy, AdminTaskSortBy, SortOrder};
use crate::services::config_service::SystemConfigValue;
use crate::services::external_auth_service::{
    CreateExternalAuthProviderInput, ExternalAuthProviderTestParamsInput,
    UpdateExternalAuthProviderInput,
};
use crate::types::{
    BackgroundTaskKind, BackgroundTaskStatus, ExternalAuthKind, SystemConfigVisibility,
};

#[derive(Debug, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct SetConfigReq {
    pub value: SystemConfigValue,
    pub visibility: Option<SystemConfigVisibility>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct RemovedCountResponse {
    pub removed: u64,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(
    all(debug_assertions, feature = "openapi"),
    derive(IntoParams, ToSchema)
)]
pub struct AdminTaskListQuery {
    pub kind: Option<BackgroundTaskKind>,
    pub status: Option<BackgroundTaskStatus>,
    pub sort_by: Option<AdminTaskSortBy>,
    pub sort_order: Option<SortOrder>,
}

impl AdminTaskListQuery {
    pub fn sort_by(&self) -> AdminTaskSortBy {
        self.sort_by.unwrap_or_default()
    }

    pub fn sort_order(&self) -> SortOrder {
        self.sort_order.unwrap_or(SortOrder::Desc)
    }
}

#[derive(Debug, Deserialize, Validate)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct AdminTaskCleanupReq {
    pub finished_before: DateTime<Utc>,
    pub kind: Option<BackgroundTaskKind>,
    pub status: Option<BackgroundTaskStatus>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(
    all(debug_assertions, feature = "openapi"),
    derive(IntoParams, ToSchema)
)]
pub struct AdminAuditLogSortQuery {
    pub sort_by: Option<AdminAuditLogSortBy>,
    pub sort_order: Option<SortOrder>,
}

impl AdminAuditLogSortQuery {
    pub fn sort_by(&self) -> AdminAuditLogSortBy {
        self.sort_by.unwrap_or(AdminAuditLogSortBy::CreatedAt)
    }

    pub fn sort_order(&self) -> SortOrder {
        self.sort_order.unwrap_or(SortOrder::Desc)
    }
}

#[derive(Debug, Deserialize, Validate)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct CreateExternalAuthProviderReq {
    #[validate(length(max = 96, message = "key must not exceed 96 bytes"))]
    pub key: Option<String>,
    #[validate(length(max = 96, message = "slug must not exceed 96 bytes"))]
    pub slug: Option<String>,
    pub kind: Option<ExternalAuthKind>,
    pub provider_kind: Option<ExternalAuthKind>,
    #[validate(custom(function = "crate::api::dto::validation::validate_non_blank"))]
    #[validate(length(max = 128, message = "display_name must not exceed 128 bytes"))]
    pub display_name: String,
    pub issuer_url: Option<String>,
    pub authorization_url: Option<String>,
    pub authorize_url: Option<String>,
    pub token_url: Option<String>,
    pub userinfo_url: Option<String>,
    #[validate(custom(function = "crate::api::dto::validation::validate_non_blank"))]
    #[validate(length(max = 255, message = "client_id must not exceed 255 bytes"))]
    pub client_id: String,
    pub client_secret: Option<String>,
    pub scopes: Option<String>,
    pub enabled: Option<bool>,
}

impl From<CreateExternalAuthProviderReq> for CreateExternalAuthProviderInput {
    fn from(value: CreateExternalAuthProviderReq) -> Self {
        Self {
            key: value.key,
            slug: value.slug,
            kind: value.kind,
            provider_kind: value.provider_kind,
            display_name: value.display_name,
            issuer_url: value.issuer_url,
            authorization_url: value.authorization_url,
            authorize_url: value.authorize_url,
            token_url: value.token_url,
            userinfo_url: value.userinfo_url,
            client_id: value.client_id,
            client_secret: value.client_secret,
            scopes: value.scopes,
            enabled: value.enabled,
        }
    }
}

#[derive(Debug, Deserialize, Validate)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct UpdateExternalAuthProviderReq {
    #[validate(length(max = 96, message = "key must not exceed 96 bytes"))]
    pub key: Option<String>,
    #[validate(length(max = 96, message = "slug must not exceed 96 bytes"))]
    pub slug: Option<String>,
    pub kind: Option<ExternalAuthKind>,
    pub provider_kind: Option<ExternalAuthKind>,
    #[validate(length(max = 128, message = "display_name must not exceed 128 bytes"))]
    pub display_name: Option<String>,
    pub issuer_url: Option<String>,
    pub authorization_url: Option<String>,
    pub authorize_url: Option<String>,
    pub token_url: Option<String>,
    pub userinfo_url: Option<String>,
    #[validate(length(max = 255, message = "client_id must not exceed 255 bytes"))]
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub scopes: Option<String>,
    pub enabled: Option<bool>,
}

impl From<UpdateExternalAuthProviderReq> for UpdateExternalAuthProviderInput {
    fn from(value: UpdateExternalAuthProviderReq) -> Self {
        Self {
            key: value.key,
            slug: value.slug,
            kind: value.kind,
            provider_kind: value.provider_kind,
            display_name: value.display_name,
            issuer_url: value.issuer_url,
            authorization_url: value.authorization_url,
            authorize_url: value.authorize_url,
            token_url: value.token_url,
            userinfo_url: value.userinfo_url,
            client_id: value.client_id,
            client_secret: value.client_secret,
            scopes: value.scopes,
            enabled: value.enabled,
        }
    }
}

#[derive(Debug, Deserialize, Validate)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct ExternalAuthProviderTestParamsReq {
    pub kind: Option<ExternalAuthKind>,
    pub provider_kind: Option<ExternalAuthKind>,
    pub issuer_url: Option<String>,
    pub authorization_url: Option<String>,
    pub authorize_url: Option<String>,
    pub token_url: Option<String>,
    pub userinfo_url: Option<String>,
    #[validate(custom(function = "crate::api::dto::validation::validate_non_blank"))]
    #[validate(length(max = 255, message = "client_id must not exceed 255 bytes"))]
    pub client_id: String,
    pub client_secret: Option<String>,
    pub scopes: Option<String>,
}

impl From<ExternalAuthProviderTestParamsReq> for ExternalAuthProviderTestParamsInput {
    fn from(value: ExternalAuthProviderTestParamsReq) -> Self {
        Self {
            kind: value.kind,
            provider_kind: value.provider_kind,
            issuer_url: value.issuer_url,
            authorization_url: value.authorization_url,
            authorize_url: value.authorize_url,
            token_url: value.token_url,
            userinfo_url: value.userinfo_url,
            client_id: value.client_id,
            client_secret: value.client_secret,
            scopes: value.scopes,
        }
    }
}
