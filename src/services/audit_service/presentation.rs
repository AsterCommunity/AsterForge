use std::collections::BTreeMap;

use serde_json::Value;

use super::models::{AuditPresentation, AuditPresentationMessage};
use crate::types::{AuditAction, AuditEntityType};

pub fn build_audit_presentation(
    action: AuditAction,
    entity_type: AuditEntityType,
    entity_id: Option<i64>,
    entity_name: Option<&str>,
    details: Option<&str>,
) -> Option<AuditPresentation> {
    let parsed_details = details.and_then(parse_details);
    let summary = Some(summary_message(
        action,
        entity_name,
        parsed_details.as_ref(),
    ));
    let target = match action {
        AuditAction::ServerStart | AuditAction::ServerShutdown => Some(server_target()),
        _ => target_message(entity_type, entity_id, entity_name),
    };
    let detail = detail_message(action, parsed_details.as_ref());

    Some(AuditPresentation {
        summary,
        target,
        detail,
    })
}

fn parse_details(raw: &str) -> Option<Value> {
    serde_json::from_str(raw).ok()
}

fn summary_message(
    action: AuditAction,
    entity_name: Option<&str>,
    details: Option<&Value>,
) -> AuditPresentationMessage {
    let mut params = BTreeMap::new();
    if let Some(name) = entity_name {
        params.insert("name".to_string(), Value::String(name.to_string()));
    }

    match action {
        AuditAction::ConfigUpdate
        | AuditAction::ConfigActionExecute
        | AuditAction::AdminDeleteConfig => {
            if let Some(name) = entity_name {
                params.insert("key".to_string(), Value::String(name.to_string()));
            }
            copy_string_param(details, &mut params, "action");
        }
        AuditAction::AdminCreateExternalAuthProvider
        | AuditAction::AdminUpdateExternalAuthProvider
        | AuditAction::AdminDeleteExternalAuthProvider
        | AuditAction::AdminTestExternalAuthProvider
        | AuditAction::ExternalAuthProviderCreate
        | AuditAction::ExternalAuthProviderUpdate
        | AuditAction::ExternalAuthProviderDelete => {
            copy_string_param(details, &mut params, "key");
            copy_string_param(details, &mut params, "slug");
        }
        _ => {}
    }

    AuditPresentationMessage {
        code: action.as_str().to_string(),
        params,
    }
}

fn target_message(
    entity_type: AuditEntityType,
    entity_id: Option<i64>,
    entity_name: Option<&str>,
) -> Option<AuditPresentationMessage> {
    if entity_id.is_none() && entity_name.is_none() {
        return None;
    }

    let mut params = BTreeMap::new();
    if let Some(id) = entity_id {
        params.insert("id".to_string(), Value::Number(id.into()));
    }
    if let Some(name) = entity_name {
        params.insert("name".to_string(), Value::String(name.to_string()));
    }

    Some(AuditPresentationMessage {
        code: entity_type.as_str().to_string(),
        params,
    })
}

fn server_target() -> AuditPresentationMessage {
    AuditPresentationMessage {
        code: "server".to_string(),
        params: BTreeMap::new(),
    }
}

fn detail_message(
    action: AuditAction,
    details: Option<&Value>,
) -> Option<AuditPresentationMessage> {
    let details = details?;
    let mut params = BTreeMap::new();

    match action {
        AuditAction::ConfigUpdate => {
            copy_param(details, &mut params, "value");
            copy_param(details, &mut params, "visibility");
            copy_param(details, &mut params, "prior_visibility");
            Some(message("config_value_updated", params))
        }
        AuditAction::ConfigActionExecute => {
            copy_param(details, &mut params, "action");
            copy_param(details, &mut params, "target_email");
            Some(message("config_action_executed", params))
        }
        AuditAction::UserLogin => {
            copy_param(details, &mut params, "identifier");
            Some(message("user_login_identifier", params))
        }
        AuditAction::AdminCleanupTasks => {
            copy_param(details, &mut params, "removed");
            copy_param(details, &mut params, "finished_before");
            copy_param(details, &mut params, "kind");
            copy_param(details, &mut params, "status");
            Some(message("tasks_cleanup_finished", params))
        }
        AuditAction::TaskRetry => {
            copy_param(details, &mut params, "kind");
            copy_param(details, &mut params, "previous_attempt_count");
            Some(message("task_retry_scheduled", params))
        }
        AuditAction::MailSend => {
            copy_param(details, &mut params, "to_address");
            copy_param(details, &mut params, "template_code");
            copy_param(details, &mut params, "outbox_id");
            Some(message("mail_sent", params))
        }
        AuditAction::MailDeliveryFailed => {
            copy_param(details, &mut params, "to_address");
            copy_param(details, &mut params, "template_code");
            copy_param(details, &mut params, "outbox_id");
            copy_param(details, &mut params, "attempt_count");
            copy_param(details, &mut params, "error");
            Some(message("mail_delivery_failed", params))
        }
        AuditAction::AdminCreateExternalAuthProvider
        | AuditAction::AdminUpdateExternalAuthProvider
        | AuditAction::AdminDeleteExternalAuthProvider
        | AuditAction::ExternalAuthProviderCreate
        | AuditAction::ExternalAuthProviderUpdate
        | AuditAction::ExternalAuthProviderDelete => {
            copy_param(details, &mut params, "key");
            copy_param(details, &mut params, "slug");
            copy_param(details, &mut params, "kind");
            copy_param(details, &mut params, "issuer_url");
            copy_param(details, &mut params, "enabled");
            Some(message("external_auth_provider_changed", params))
        }
        AuditAction::AdminTestExternalAuthProvider => {
            copy_param(details, &mut params, "provider");
            copy_param(details, &mut params, "key");
            copy_param(details, &mut params, "success");
            copy_param(details, &mut params, "slug");
            copy_param(details, &mut params, "kind");
            copy_param(details, &mut params, "issuer_url");
            copy_param(details, &mut params, "enabled");
            Some(message("external_auth_provider_tested", params))
        }
        _ => None,
    }
}

fn message(code: &str, params: BTreeMap<String, Value>) -> AuditPresentationMessage {
    AuditPresentationMessage {
        code: code.to_string(),
        params,
    }
}

fn copy_string_param(source: Option<&Value>, params: &mut BTreeMap<String, Value>, key: &str) {
    let Some(value) = source
        .and_then(|source| source.get(key))
        .and_then(Value::as_str)
    else {
        return;
    };
    params.insert(key.to_string(), Value::String(value.to_string()));
}

fn copy_param(source: &Value, params: &mut BTreeMap<String, Value>, key: &str) {
    let Some(value) = source.get(key) else {
        return;
    };
    if value.is_null() {
        return;
    }
    params.insert(key.to_string(), value.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_includes_config_key_and_value_detail() {
        let presentation = build_audit_presentation(
            AuditAction::ConfigUpdate,
            AuditEntityType::SystemConfig,
            Some(42),
            Some("audit_log_recorded_actions"),
            Some(r#"{"value":"[\"user_login\"]","visibility":"private"}"#),
        )
        .expect("presentation should be built");

        assert_eq!(presentation.summary.as_ref().unwrap().code, "config_update");
        assert_eq!(
            presentation.summary.as_ref().unwrap().params.get("key"),
            Some(&Value::String("audit_log_recorded_actions".to_string()))
        );
        assert_eq!(
            presentation.detail.as_ref().unwrap().code,
            "config_value_updated"
        );
    }

    #[test]
    fn presentation_includes_config_action_detail() {
        let presentation = build_audit_presentation(
            AuditAction::ConfigActionExecute,
            AuditEntityType::SystemConfig,
            None,
            Some("mail"),
            Some(r#"{"action":"send_test_email","target_email":"admin@example.com"}"#),
        )
        .expect("presentation should be built");

        assert_eq!(
            presentation.summary.as_ref().unwrap().code,
            "config_action_execute"
        );
        assert_eq!(
            presentation.summary.as_ref().unwrap().params.get("key"),
            Some(&Value::String("mail".to_string()))
        );
        assert_eq!(
            presentation.detail.as_ref().unwrap().code,
            "config_action_executed"
        );
        assert_eq!(
            presentation
                .detail
                .as_ref()
                .unwrap()
                .params
                .get("target_email"),
            Some(&Value::String("admin@example.com".to_string()))
        );
    }

    #[test]
    fn presentation_includes_mail_delivery_detail() {
        let presentation = build_audit_presentation(
            AuditAction::MailDeliveryFailed,
            AuditEntityType::System,
            None,
            Some("mail"),
            Some(
                r#"{"to_address":"user@example.com","template_code":"password_reset","outbox_id":7,"attempt_count":2,"error":"smtp timeout"}"#,
            ),
        )
        .expect("presentation should be built");

        assert_eq!(
            presentation.detail.as_ref().unwrap().code,
            "mail_delivery_failed"
        );
        assert_eq!(
            presentation
                .detail
                .as_ref()
                .unwrap()
                .params
                .get("to_address"),
            Some(&Value::String("user@example.com".to_string()))
        );
    }

    #[test]
    fn presentation_handles_malformed_details_with_safe_fallback_fields() {
        let presentation = build_audit_presentation(
            AuditAction::UserLogin,
            AuditEntityType::AuthSession,
            Some(7),
            Some("admin"),
            Some("not json"),
        )
        .expect("presentation should be built");

        assert_eq!(presentation.summary.as_ref().unwrap().code, "user_login");
        assert!(presentation.detail.is_none());
        assert_eq!(presentation.target.as_ref().unwrap().code, "auth_session");
    }

    #[test]
    fn presentation_uses_server_target_for_server_lifecycle_actions() {
        let presentation = build_audit_presentation(
            AuditAction::ServerStart,
            AuditEntityType::System,
            None,
            None,
            None,
        )
        .expect("presentation should be built");

        assert_eq!(presentation.summary.as_ref().unwrap().code, "server_start");
        assert_eq!(presentation.target.as_ref().unwrap().code, "server");
        assert!(presentation.detail.is_none());
    }
}
