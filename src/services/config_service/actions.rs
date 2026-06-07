use crate::config::mail;
use crate::db::repository::user_repo;
use crate::errors::{AsterError, Result};
use crate::runtime::MailRuntimeState;
use crate::services::{
    audit_service::{self, AuditContext},
    mail_audit_service, mail_service,
};
use serde::{Deserialize, Serialize};
#[cfg(all(debug_assertions, feature = "openapi"))]
use utoipa::ToSchema;

pub const MAIL_CONFIG_ACTION_KEY: &str = "mail";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub enum ConfigActionType {
    SendTestEmail,
}

impl ConfigActionType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SendTestEmail => "send_test_email",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigActionResult {
    pub message: String,
    pub target_email: Option<String>,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct ExecuteConfigActionInput<'a> {
    pub key: &'a str,
    pub action: ConfigActionType,
    pub actor_user_id: i64,
    pub target_email: Option<&'a str>,
}

pub async fn execute_action_with_audit(
    state: &impl MailRuntimeState,
    input: ExecuteConfigActionInput<'_>,
    audit_ctx: &AuditContext,
) -> Result<ConfigActionResult> {
    let action_result = match input.key {
        MAIL_CONFIG_ACTION_KEY => execute_mail_action(state, input, audit_ctx).await,
        key => Err(AsterError::record_not_found(format!(
            "config action target '{key}'"
        ))),
    }?;
    audit_service::log_with_details(
        state,
        audit_ctx,
        audit_service::AuditAction::ConfigActionExecute,
        audit_service::AuditEntityType::SystemConfig,
        None,
        Some(input.key),
        || {
            audit_service::details(audit_service::ConfigActionDetails {
                action: input.action.as_str(),
                target_email: action_result.target_email.as_deref(),
            })
        },
    )
    .await;
    Ok(action_result)
}

async fn execute_mail_action(
    state: &impl MailRuntimeState,
    input: ExecuteConfigActionInput<'_>,
    audit_ctx: &AuditContext,
) -> Result<ConfigActionResult> {
    match input.action {
        ConfigActionType::SendTestEmail => {
            let actor = user_repo::find_by_id(state.reader_db(), input.actor_user_id).await?;
            let requested_target = input.target_email.unwrap_or(&actor.email);
            let normalized_target = mail::normalize_mail_address_config_value(requested_target)?;
            if normalized_target.is_empty() {
                return Err(AsterError::validation_error("target_email is required"));
            }

            tracing::debug!(
                actor_user_id = input.actor_user_id,
                actor_username = %actor.username,
                target_email = %normalized_target,
                action = %input.action.as_str(),
                "config: executing mail action"
            );

            let result =
                mail_service::send_test_email(state, &normalized_target, Some(&actor.username))
                    .await;
            let ip_address = audit_ctx.ip_address.as_deref();
            let user_agent = audit_ctx.user_agent.as_deref();
            match &result {
                Ok(()) => {
                    mail_audit_service::log_send(
                        state,
                        mail_audit_service::MailAuditInput {
                            actor_user_id: input.actor_user_id,
                            ip_address,
                            user_agent,
                            to_address: &normalized_target,
                            to_name: None,
                            template_code: "smtp_test",
                            subject: Some("AsterForge SMTP test"),
                            outbox_id: None,
                            attempt_count: None,
                            error: None,
                        },
                    )
                    .await;
                }
                Err(error) => {
                    let error_message = error.to_string();
                    mail_audit_service::log_delivery_failed_with_db(
                        state.writer_db(),
                        state.runtime_config(),
                        mail_audit_service::MailAuditInput {
                            actor_user_id: input.actor_user_id,
                            ip_address,
                            user_agent,
                            to_address: &normalized_target,
                            to_name: None,
                            template_code: "smtp_test",
                            subject: Some("AsterForge SMTP test"),
                            outbox_id: None,
                            attempt_count: None,
                            error: Some(&error_message),
                        },
                    )
                    .await;
                }
            }
            result?;

            Ok(ConfigActionResult {
                message: format!("Test email sent to {normalized_target}"),
                target_email: Some(normalized_target),
                value: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ConfigActionType, MAIL_CONFIG_ACTION_KEY};

    #[test]
    fn config_action_type_exposes_stable_wire_value() {
        assert_eq!(ConfigActionType::SendTestEmail.as_str(), "send_test_email");
        assert_eq!(MAIL_CONFIG_ACTION_KEY, "mail");
    }
}
