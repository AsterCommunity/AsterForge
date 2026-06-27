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

#[cfg(feature = "runtime-component")]
mod component;
pub mod config;
pub mod message;
pub mod outbox;
pub mod sender;
pub mod template;

/// Stable component name used for mail outbox lifecycle handling.
pub const MAIL_OUTBOX_COMPONENT: &str = "mail_outbox";

#[cfg(feature = "runtime-component")]
pub use component::{MAIL_OUTBOX_DRAIN_SHUTDOWN_PHASE, mail_outbox_component};
pub use config::{
    DEFAULT_MAIL_SECURITY, DEFAULT_MAIL_SMTP_PORT, MAIL_TEMPLATE_MAX_BODY_LEN,
    MAIL_TEMPLATE_MAX_SUBJECT_LEN, MailConfigError, MailConfigResult, MailRuntimeSettings,
    normalize_mail_address_config_value, normalize_mail_name_config_value,
    normalize_mail_security_config_value, normalize_mail_template_body_config_value,
    normalize_mail_template_subject_config_value, normalize_smtp_host_config_value,
    normalize_smtp_port_config_value, parse_smtp_port,
};
pub use message::{MailMessage, MailRecipient};
pub use outbox::{
    DEFAULT_ERROR_MAX_LEN, DEFAULT_MARK_SENT_RETRY_DELAYS_MS, DispatchStats,
    MailOutboxDeliveryFailureDecision, MailOutboxDispatchConfig, MailOutboxDispatchRow,
    MailOutboxRetryPolicy, MailOutboxStatus, MailTemplateCode, StoredMailPayload,
    dispatch_mail_outbox, drain_mail_outbox, retry_mark_sent, truncate_error,
};
pub use sender::{
    DEFAULT_SMTP_SEND_TIMEOUT_SECS, MailDeliveryError, MailSendResult, MailSender,
    MemoryMailSender, SmtpMailSender, memory_sender, memory_sender_ref, send_rendered_with,
    smtp_sender,
};
pub use template::{
    MailTemplateCatalog, MailTemplateCatalogBuilder, MailTemplateDefinition, MailTemplateRegistrar,
    MailTemplateRegistry, MailTemplateRegistryError, RenderedMail, TemplatePlaceholderSet,
    TemplateVariableGroup, TemplateVariableItem, TemplateVariableSpec, escape_html, html_to_text,
    render_placeholders, render_template,
};
