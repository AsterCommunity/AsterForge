use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::types::{BackgroundTaskKind, BackgroundTaskStatus, SystemConfigVisibility};

#[derive(Serialize)]
pub struct ConfigUpdateDetails<'a> {
    pub value: &'a str,
    pub visibility: SystemConfigVisibility,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prior_visibility: Option<SystemConfigVisibility>,
}

#[derive(Serialize)]
pub struct ConfigActionDetails<'a> {
    pub action: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_email: Option<&'a str>,
}

#[derive(Serialize)]
pub struct MailAuditDetails<'a> {
    pub to_address: &'a str,
    pub template_code: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outbox_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<&'a str>,
}

#[derive(Serialize)]
pub struct AdminTaskCleanupAuditDetails {
    pub removed: u64,
    pub finished_before: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<BackgroundTaskKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<BackgroundTaskStatus>,
}

#[derive(Serialize)]
pub struct TaskRetryAuditDetails {
    pub kind: String,
    pub previous_attempt_count: i32,
}

#[derive(Serialize)]
pub struct LoginAuditDetails<'a> {
    pub identifier: &'a str,
}

#[derive(Serialize)]
pub struct ExternalAuthProviderTestParamsAuditDetails<'a> {
    pub provider: &'a str,
    pub key: &'a str,
    pub success: bool,
}

pub fn details<T: Serialize>(value: T) -> Option<serde_json::Value> {
    match serde_json::to_value(value) {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::warn!("failed to serialize audit details: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AdminTaskCleanupAuditDetails, ConfigActionDetails, ConfigUpdateDetails, LoginAuditDetails,
        MailAuditDetails, TaskRetryAuditDetails, details,
    };
    use crate::types::{BackgroundTaskKind, BackgroundTaskStatus, SystemConfigVisibility};
    use chrono::Utc;

    #[test]
    fn details_serializes_config_update_and_omits_missing_prior_visibility() {
        assert_eq!(
            details(ConfigUpdateDetails {
                value: "public title",
                visibility: SystemConfigVisibility::Public,
                prior_visibility: None,
            })
            .unwrap(),
            serde_json::json!({
                "value": "public title",
                "visibility": "public",
            })
        );

        assert_eq!(
            details(ConfigUpdateDetails {
                value: "***REDACTED***",
                visibility: SystemConfigVisibility::Authenticated,
                prior_visibility: Some(SystemConfigVisibility::Private),
            })
            .unwrap(),
            serde_json::json!({
                "value": "***REDACTED***",
                "visibility": "authenticated",
                "prior_visibility": "private",
            })
        );
    }

    #[test]
    fn details_serializes_config_action_and_omits_missing_target_email() {
        assert_eq!(
            details(ConfigActionDetails {
                action: "send_test_email",
                target_email: Some("admin@example.com"),
            })
            .unwrap(),
            serde_json::json!({
                "action": "send_test_email",
                "target_email": "admin@example.com",
            })
        );

        assert_eq!(
            details(ConfigActionDetails {
                action: "send_test_email",
                target_email: None,
            })
            .unwrap(),
            serde_json::json!({
                "action": "send_test_email",
            })
        );
    }

    #[test]
    fn details_serializes_task_cleanup_and_retry_shapes() {
        let finished_before = Utc::now();
        assert_eq!(
            details(AdminTaskCleanupAuditDetails {
                removed: 3,
                finished_before,
                kind: Some(BackgroundTaskKind::SystemRuntime),
                status: Some(BackgroundTaskStatus::Failed),
            })
            .unwrap(),
            serde_json::json!({
                "removed": 3,
                "finished_before": finished_before,
                "kind": "system_runtime",
                "status": "failed",
            })
        );

        assert_eq!(
            details(TaskRetryAuditDetails {
                kind: "system_runtime".to_string(),
                previous_attempt_count: 2,
            })
            .unwrap(),
            serde_json::json!({
                "kind": "system_runtime",
                "previous_attempt_count": 2,
            })
        );
    }

    #[test]
    fn details_serializes_mail_audit_shape() {
        assert_eq!(
            details(MailAuditDetails {
                to_address: "user@example.com",
                template_code: "password_reset",
                to_name: Some("User"),
                subject: Some("Reset password"),
                outbox_id: Some(42),
                attempt_count: Some(2),
                error: Some("smtp timeout"),
            })
            .unwrap(),
            serde_json::json!({
                "to_address": "user@example.com",
                "template_code": "password_reset",
                "to_name": "User",
                "subject": "Reset password",
                "outbox_id": 42,
                "attempt_count": 2,
                "error": "smtp timeout",
            })
        );
    }

    #[test]
    fn details_serializes_login_identifier() {
        assert_eq!(
            details(LoginAuditDetails {
                identifier: "admin@example.com",
            })
            .unwrap(),
            serde_json::json!({ "identifier": "admin@example.com" })
        );
    }
}
