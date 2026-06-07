//! Background task retry policy.

use crate::errors::AsterError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskRetryClass {
    Auto,
    Manual,
    Never,
}

impl TaskRetryClass {
    pub(super) fn should_auto_retry(self) -> bool {
        matches!(self, Self::Auto)
    }

    pub(super) fn can_manual_retry(self) -> bool {
        matches!(self, Self::Auto | Self::Manual)
    }
}

pub(super) fn default_retry_class(error: &AsterError) -> TaskRetryClass {
    match error {
        AsterError::DatabaseConnection(_) | AsterError::MailDeliveryFailed(_) => {
            TaskRetryClass::Auto
        }
        AsterError::DatabaseOperation(_)
        | AsterError::ConfigError(_)
        | AsterError::ExternalAuthError(_)
        | AsterError::InternalError(_) => TaskRetryClass::Manual,
        AsterError::ValidationError(_)
        | AsterError::AuthInvalidCredentials(_)
        | AsterError::AuthTokenInvalid(_)
        | AsterError::AuthTokenExpired(_)
        | AsterError::AuthForbidden(_)
        | AsterError::RecordNotFound(_)
        | AsterError::MailNotConfigured(_) => TaskRetryClass::Never,
    }
}

#[cfg(test)]
mod tests {
    use super::{TaskRetryClass, default_retry_class};
    use crate::errors::AsterError;

    #[test]
    fn retry_class_helpers_match_retry_capabilities() {
        assert!(TaskRetryClass::Auto.should_auto_retry());
        assert!(TaskRetryClass::Auto.can_manual_retry());

        assert!(!TaskRetryClass::Manual.should_auto_retry());
        assert!(TaskRetryClass::Manual.can_manual_retry());

        assert!(!TaskRetryClass::Never.should_auto_retry());
        assert!(!TaskRetryClass::Never.can_manual_retry());
    }

    #[test]
    fn default_retry_class_groups_transient_manual_and_permanent_errors() {
        assert_eq!(
            default_retry_class(&AsterError::database_connection("connect failed")),
            TaskRetryClass::Auto
        );
        assert_eq!(
            default_retry_class(&AsterError::mail_delivery_failed("smtp timeout")),
            TaskRetryClass::Auto
        );

        for error in [
            AsterError::database_operation("query failed"),
            AsterError::config_error("config failed"),
            AsterError::external_auth_error("provider failed"),
            AsterError::internal_error("internal failed"),
        ] {
            assert_eq!(default_retry_class(&error), TaskRetryClass::Manual);
        }

        for error in [
            AsterError::validation_error("bad input"),
            AsterError::auth_invalid_credentials("bad credentials"),
            AsterError::auth_token_invalid("bad token"),
            AsterError::auth_token_expired("expired token"),
            AsterError::auth_forbidden("forbidden"),
            AsterError::record_not_found("missing"),
            AsterError::mail_not_configured("smtp missing"),
        ] {
            assert_eq!(default_retry_class(&error), TaskRetryClass::Never);
        }
    }
}
