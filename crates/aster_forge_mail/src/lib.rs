//! Shared mail infrastructure helpers for Aster services.
//!
//! This crate intentionally does not own product templates, recipients, audit
//! records, user context, SMTP configuration keys, or database entities. It only
//! provides small mechanics that recur around mail outbox dispatch: dispatch
//! counters, retry delay selection, error truncation, and best-effort retry when
//! persisting a successful SMTP delivery. It also provides a product-neutral
//! template registry and placeholder rendering helpers for services that own
//! their own template codes and payloads.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )
)]

pub mod config;
pub mod message;
pub mod outbox;
pub mod template;

pub use config::{
    MAIL_TEMPLATE_MAX_BODY_LEN, MAIL_TEMPLATE_MAX_SUBJECT_LEN, MailConfigError, MailConfigResult,
    normalize_mail_address_config_value, normalize_mail_name_config_value,
    normalize_mail_security_config_value, normalize_mail_template_body_config_value,
    normalize_mail_template_subject_config_value, normalize_smtp_host_config_value,
    normalize_smtp_port_config_value, parse_smtp_port,
};
pub use message::{MailMessage, MailRecipient};
pub use outbox::{
    DEFAULT_ERROR_MAX_LEN, DEFAULT_MARK_SENT_RETRY_DELAYS_MS, DispatchStats,
    MailOutboxDeliveryFailureDecision, MailOutboxRetryPolicy, retry_mark_sent, truncate_error,
};
pub use template::{
    MailTemplateCatalog, MailTemplateCatalogBuilder, MailTemplateDefinition, MailTemplateRegistry,
    MailTemplateRegistryError, RenderedMail, TemplatePlaceholderSet, TemplateVariableGroup,
    TemplateVariableItem, TemplateVariableSpec, escape_html, html_to_text, render_placeholders,
    render_template,
};
