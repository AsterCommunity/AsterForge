//! Product-neutral mail runtime configuration normalization.
//!
//! Product crates still own the concrete configuration keys, default values, runtime config
//! reading, and API error mapping. This module keeps recurring validation and normalization rules
//! for mail-related config values, plus the product-neutral runtime settings model used by shared
//! sender implementations.

use std::error::Error;
use std::fmt;

use aster_forge_utils::bool_like::parse_bool_like;
use aster_forge_validation::email::normalize_email;

/// Maximum subject length accepted by the shared mail template normalizer.
pub const MAIL_TEMPLATE_MAX_SUBJECT_LEN: usize = 255;

/// Maximum HTML body length accepted by the shared mail template normalizer.
pub const MAIL_TEMPLATE_MAX_BODY_LEN: usize = 64 * 1024;

/// Default SMTP port used by Aster services when runtime config is absent.
pub const DEFAULT_MAIL_SMTP_PORT: u16 = 587;

/// Default SMTP encryption policy used by Aster services when runtime config is absent.
pub const DEFAULT_MAIL_SECURITY: bool = true;

/// Runtime SMTP settings shared by Aster service mail senders.
///
/// Product crates still own config keys, persistence, validation error mapping,
/// and transport error mapping. This struct only keeps the repeated SMTP
/// readiness rules and sender envelope values in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailRuntimeSettings {
    /// SMTP relay host.
    pub smtp_host: String,
    /// SMTP relay port.
    pub smtp_port: u16,
    /// Optional SMTP username.
    pub smtp_username: String,
    /// Optional SMTP password.
    pub smtp_password: String,
    /// Sender email address.
    pub from_address: String,
    /// Sender display name.
    pub from_name: String,
    /// Whether TLS/STARTTLS transport should be used.
    pub encryption_enabled: bool,
}

impl MailRuntimeSettings {
    /// Returns whether the minimum outbound mail settings are configured.
    pub fn is_configured(&self) -> bool {
        !self.smtp_host.trim().is_empty() && !self.from_address.trim().is_empty()
    }

    /// Returns whether settings are ready for a delivery attempt.
    ///
    /// The SMTP auth fields are intentionally all-or-nothing so products do not
    /// accidentally attempt passwordless auth or send a password without a user.
    pub fn is_ready_for_delivery(&self) -> bool {
        self.is_configured()
            && self.smtp_username.trim().is_empty() == self.smtp_password.trim().is_empty()
    }
}

/// Error returned when mail configuration normalization fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailConfigError {
    message: String,
}

impl MailConfigError {
    /// Creates a mail configuration validation error.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Returns the validation failure message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for MailConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for MailConfigError {}

/// Result type returned by shared mail configuration helpers.
pub type MailConfigResult<T> = std::result::Result<T, MailConfigError>;

/// Parses an SMTP port from a storage string.
pub fn parse_smtp_port(value: &str) -> Option<u16> {
    value.trim().parse::<u16>().ok().filter(|port| *port > 0)
}

/// Normalizes an SMTP host value.
///
/// Empty values are allowed so products can represent "mail is not configured"
/// without introducing product-specific sentinel values.
pub fn normalize_smtp_host_config_value(value: &str) -> MailConfigResult<String> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Ok(String::new());
    }
    if normalized.contains(char::is_whitespace) {
        return Err(MailConfigError::new("mail_smtp_host cannot contain spaces"));
    }
    Ok(normalized)
}

/// Normalizes an SMTP port value.
pub fn normalize_smtp_port_config_value(value: &str) -> MailConfigResult<String> {
    let Some(port) = parse_smtp_port(value) else {
        return Err(MailConfigError::new(
            "mail_smtp_port must be an integer between 1 and 65535",
        ));
    };
    Ok(port.to_string())
}

/// Normalizes a sender email address value.
///
/// Empty values are allowed so products can leave outbound mail disabled until
/// an operator configures both SMTP host and sender address.
pub fn normalize_mail_address_config_value(value: &str) -> MailConfigResult<String> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Ok(String::new());
    }
    normalize_email(&normalized).map_err(|error| MailConfigError::new(error.to_string()))
}

/// Normalizes a sender display name value.
pub fn normalize_mail_name_config_value(value: &str) -> MailConfigResult<String> {
    let normalized = value.trim();
    if normalized.len() > 128 {
        return Err(MailConfigError::new(
            "mail_from_name must be at most 128 characters",
        ));
    }
    Ok(normalized.to_string())
}

/// Normalizes a bool-like mail security config value.
pub fn normalize_mail_security_config_value(value: &str) -> MailConfigResult<String> {
    match parse_bool_like(value) {
        Some(value) => Ok(if value { "true" } else { "false" }.to_string()),
        None => Err(MailConfigError::new(
            "mail_security must be 'true' or 'false'",
        )),
    }
}

/// Normalizes a mail template subject.
pub fn normalize_mail_template_subject_config_value(
    key: &str,
    value: &str,
) -> MailConfigResult<String> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Err(MailConfigError::new(format!("{key} cannot be empty")));
    }
    if normalized.contains(['\r', '\n']) {
        return Err(MailConfigError::new(format!("{key} must be a single line")));
    }
    if normalized.len() > MAIL_TEMPLATE_MAX_SUBJECT_LEN {
        return Err(MailConfigError::new(format!(
            "{key} must be at most {MAIL_TEMPLATE_MAX_SUBJECT_LEN} characters",
        )));
    }
    Ok(normalized.to_string())
}

/// Normalizes a mail template HTML body.
pub fn normalize_mail_template_body_config_value(
    key: &str,
    value: &str,
) -> MailConfigResult<String> {
    let normalized = normalize_multiline(value);
    if normalized.trim().is_empty() {
        return Err(MailConfigError::new(format!("{key} cannot be empty")));
    }
    if normalized.len() > MAIL_TEMPLATE_MAX_BODY_LEN {
        return Err(MailConfigError::new(format!(
            "{key} must be at most {MAIL_TEMPLATE_MAX_BODY_LEN} characters",
        )));
    }
    Ok(normalized)
}

fn normalize_multiline(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_MAIL_SECURITY, DEFAULT_MAIL_SMTP_PORT, MailRuntimeSettings,
        normalize_mail_address_config_value, normalize_mail_name_config_value,
        normalize_mail_security_config_value, normalize_mail_template_body_config_value,
        normalize_mail_template_subject_config_value, normalize_smtp_host_config_value,
        normalize_smtp_port_config_value, parse_smtp_port,
    };

    #[test]
    fn smtp_host_normalizer_allows_empty_and_rejects_spaces() {
        assert_eq!(normalize_smtp_host_config_value("  ").unwrap(), "");
        assert_eq!(
            normalize_smtp_host_config_value(" SMTP.Example.COM ").unwrap(),
            "smtp.example.com"
        );
        assert!(normalize_smtp_host_config_value("smtp example.com").is_err());
    }

    #[test]
    fn smtp_port_normalizer_accepts_valid_ports_only() {
        assert_eq!(parse_smtp_port("587"), Some(587));
        assert_eq!(parse_smtp_port("0"), None);
        assert_eq!(parse_smtp_port("65536"), None);
        assert_eq!(normalize_smtp_port_config_value(" 465 ").unwrap(), "465");
        assert!(normalize_smtp_port_config_value("0").is_err());
    }

    #[test]
    fn mail_runtime_settings_report_readiness() {
        let mut settings = MailRuntimeSettings {
            smtp_host: "smtp.example.com".to_string(),
            smtp_port: DEFAULT_MAIL_SMTP_PORT,
            smtp_username: String::new(),
            smtp_password: String::new(),
            from_address: "ops@example.com".to_string(),
            from_name: "Aster Ops".to_string(),
            encryption_enabled: DEFAULT_MAIL_SECURITY,
        };
        assert!(settings.is_configured());
        assert!(settings.is_ready_for_delivery());

        settings.smtp_password = "secret".to_string();
        assert!(!settings.is_ready_for_delivery());

        settings.smtp_username = "ops".to_string();
        assert!(settings.is_ready_for_delivery());

        settings.smtp_host.clear();
        assert!(!settings.is_configured());
        assert!(!settings.is_ready_for_delivery());
    }

    #[test]
    fn mail_address_normalizer_allows_empty_and_validates_email_shape() {
        assert_eq!(normalize_mail_address_config_value("  ").unwrap(), "");
        assert_eq!(
            normalize_mail_address_config_value(" Ops@Example.COM ").unwrap(),
            "ops@example.com"
        );
        assert!(normalize_mail_address_config_value("ops@example").is_err());
    }

    #[test]
    fn mail_name_normalizer_trims_and_limits_length() {
        assert_eq!(
            normalize_mail_name_config_value("  Aster Ops  ").unwrap(),
            "Aster Ops"
        );
        assert!(normalize_mail_name_config_value(&"a".repeat(129)).is_err());
    }

    #[test]
    fn mail_security_normalizer_accepts_bool_like_values() {
        assert_eq!(
            normalize_mail_security_config_value(" yes ").unwrap(),
            "true"
        );
        assert_eq!(
            normalize_mail_security_config_value("OFF").unwrap(),
            "false"
        );
        assert!(normalize_mail_security_config_value("sometimes").is_err());
    }

    #[test]
    fn template_subject_normalizer_rejects_empty_multiline_and_long_values() {
        assert_eq!(
            normalize_mail_template_subject_config_value("subject", "  Hello  ").unwrap(),
            "Hello"
        );
        assert!(normalize_mail_template_subject_config_value("subject", "  ").is_err());
        assert!(normalize_mail_template_subject_config_value("subject", "hello\nworld").is_err());
        assert!(normalize_mail_template_subject_config_value("subject", &"a".repeat(256)).is_err());
    }

    #[test]
    fn template_body_normalizer_converts_crlf_and_enforces_limits() {
        assert_eq!(
            normalize_mail_template_body_config_value("body", "line1\r\nline2").unwrap(),
            "line1\nline2"
        );
        assert!(normalize_mail_template_body_config_value("body", "  ").is_err());
        assert!(
            normalize_mail_template_body_config_value("body", &"a".repeat(64 * 1024 + 1)).is_err()
        );
    }
}
