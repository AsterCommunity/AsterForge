use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::BTreeMap;
#[cfg(all(debug_assertions, feature = "openapi"))]
use utoipa::ToSchema;

use crate::entities::user;
use crate::types::{AuditAction, AuditEntityType, UserRole, UserStatus};

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct AuditUserSummary {
    pub id: i64,
    pub username: String,
    pub email: String,
    pub role: UserRole,
    pub status: UserStatus,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct AuditLogEntry {
    pub id: i64,
    pub user_id: i64,
    pub user: Option<AuditUserSummary>,
    pub action: AuditAction,
    pub entity_type: AuditEntityType,
    pub entity_id: Option<i64>,
    pub entity_name: Option<String>,
    pub details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presentation: Option<AuditPresentation>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = String))]
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct AuditPresentationMessage {
    pub code: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct AuditPresentation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<AuditPresentationMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<AuditPresentationMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<AuditPresentationMessage>,
}

impl From<user::Model> for AuditUserSummary {
    fn from(model: user::Model) -> Self {
        Self {
            id: model.id,
            username: model.username,
            email: model.email,
            role: model.role,
            status: model.status,
        }
    }
}
