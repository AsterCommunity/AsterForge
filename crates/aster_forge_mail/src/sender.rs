//! Product-neutral mail sender implementations.
//!
//! This module owns the repeated mechanics for in-memory test delivery and SMTP delivery through
//! `lettre`. Product crates still decide how runtime settings are read, how delivery errors map to
//! their API error model, which test email copy they send, and how deliveries are audited.

use std::any::Any;
use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use lettre::message::{Mailbox, MultiPart, SinglePart, header::ContentType};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Address, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use tokio::time::timeout;

use crate::{MailMessage, MailRecipient, MailRuntimeSettings, RenderedMail};

/// Default timeout applied to one SMTP send attempt.
pub const DEFAULT_SMTP_SEND_TIMEOUT_SECS: u64 = 15;

/// Error returned by shared mail senders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailDeliveryError {
    /// Required runtime mail settings are missing.
    NotConfigured(String),
    /// The message envelope or body is invalid.
    InvalidMessage(String),
    /// SMTP transport configuration failed.
    Config(String),
    /// SMTP transport attempted delivery and failed.
    Delivery(String),
    /// Local sender state failed.
    Internal(String),
}

impl fmt::Display for MailDeliveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConfigured(message)
            | Self::InvalidMessage(message)
            | Self::Config(message)
            | Self::Delivery(message)
            | Self::Internal(message) => formatter.write_str(message),
        }
    }
}

impl Error for MailDeliveryError {}

/// Result type returned by shared mail senders.
pub type MailSendResult<T> = std::result::Result<T, MailDeliveryError>;

/// Product-neutral mail sender trait.
#[async_trait]
pub trait MailSender: Send + Sync {
    /// Sends a fully rendered message.
    async fn send(&self, message: MailMessage) -> MailSendResult<()>;

    /// Exposes concrete sender type for test downcasting.
    fn as_any(&self) -> &dyn Any;
}

/// Creates an in-memory sender for tests and local service wiring.
pub fn memory_sender() -> Arc<dyn MailSender> {
    Arc::new(MemoryMailSender::default())
}

/// Downcasts a sender reference to [`MemoryMailSender`].
pub fn memory_sender_ref(sender: &Arc<dyn MailSender>) -> Option<&MemoryMailSender> {
    sender.as_ref().as_any().downcast_ref::<MemoryMailSender>()
}

/// Creates an SMTP sender backed by a runtime settings provider.
pub fn smtp_sender<F>(settings_provider: F) -> Arc<dyn MailSender>
where
    F: Fn() -> MailRuntimeSettings + Send + Sync + 'static,
{
    Arc::new(SmtpMailSender::new(settings_provider))
}

/// In-memory sender that stores sent messages.
#[derive(Default)]
pub struct MemoryMailSender {
    outbox: Mutex<Vec<MailMessage>>,
}

impl MemoryMailSender {
    /// Returns all messages captured by this sender.
    pub fn messages(&self) -> Vec<MailMessage> {
        match self.outbox.lock() {
            Ok(outbox) => outbox.clone(),
            Err(error) => {
                tracing::error!(%error, "memory mail sender lock poisoned");
                Vec::new()
            }
        }
    }

    /// Returns the last captured message.
    pub fn last_message(&self) -> Option<MailMessage> {
        match self.outbox.lock() {
            Ok(outbox) => outbox.last().cloned(),
            Err(error) => {
                tracing::error!(%error, "memory mail sender lock poisoned");
                None
            }
        }
    }
}

#[async_trait]
impl MailSender for MemoryMailSender {
    async fn send(&self, message: MailMessage) -> MailSendResult<()> {
        self.outbox
            .lock()
            .map_err(|error| {
                MailDeliveryError::Internal(format!("memory mail sender poisoned: {error}"))
            })?
            .push(message);
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// SMTP sender that reads runtime settings immediately before each delivery.
pub struct SmtpMailSender<F>
where
    F: Fn() -> MailRuntimeSettings + Send + Sync + 'static,
{
    settings_provider: F,
    timeout: Duration,
}

impl<F> SmtpMailSender<F>
where
    F: Fn() -> MailRuntimeSettings + Send + Sync + 'static,
{
    /// Creates an SMTP sender using the default send timeout.
    pub fn new(settings_provider: F) -> Self {
        Self {
            settings_provider,
            timeout: Duration::from_secs(DEFAULT_SMTP_SEND_TIMEOUT_SECS),
        }
    }

    /// Creates an SMTP sender with a custom send timeout.
    pub fn with_timeout(settings_provider: F, timeout: Duration) -> Self {
        Self {
            settings_provider,
            timeout,
        }
    }
}

#[async_trait]
impl<F> MailSender for SmtpMailSender<F>
where
    F: Fn() -> MailRuntimeSettings + Send + Sync + 'static,
{
    async fn send(&self, message: MailMessage) -> MailSendResult<()> {
        let settings = (self.settings_provider)();
        validate_runtime_settings(&settings)?;

        let to_address = message.to.address.clone();
        let subject = message.subject.clone();
        tracing::debug!(
            smtp_host = %settings.smtp_host,
            smtp_port = settings.smtp_port,
            encryption_enabled = settings.encryption_enabled,
            to = %to_address,
            subject = %subject,
            timeout_secs = self.timeout.as_secs(),
            "mail: preparing runtime SMTP delivery"
        );

        let email = build_lettre_message(message)?;
        let mailer = build_transport(&settings)?;
        match timeout(self.timeout, mailer.send(email)).await {
            Ok(Ok(_)) => {
                tracing::debug!(
                    smtp_host = %settings.smtp_host,
                    smtp_port = settings.smtp_port,
                    to = %to_address,
                    subject = %subject,
                    timeout_secs = self.timeout.as_secs(),
                    "mail: SMTP delivery completed"
                );
                Ok(())
            }
            Ok(Err(error)) => {
                tracing::debug!(
                    smtp_host = %settings.smtp_host,
                    smtp_port = settings.smtp_port,
                    to = %to_address,
                    subject = %subject,
                    error = %error,
                    timeout_secs = self.timeout.as_secs(),
                    "mail: SMTP delivery failed"
                );
                Err(MailDeliveryError::Delivery(error.to_string()))
            }
            Err(_) => {
                tracing::debug!(
                    smtp_host = %settings.smtp_host,
                    smtp_port = settings.smtp_port,
                    to = %to_address,
                    subject = %subject,
                    timeout_secs = self.timeout.as_secs(),
                    "mail: SMTP delivery timed out"
                );
                Err(MailDeliveryError::Delivery(format!(
                    "mail delivery timed out after {} seconds",
                    self.timeout.as_secs()
                )))
            }
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Sends a rendered message through a sender using the provided runtime settings for the sender
/// envelope.
pub async fn send_rendered_with(
    mail_sender: &Arc<dyn MailSender>,
    settings: &MailRuntimeSettings,
    to: MailRecipient,
    rendered: RenderedMail,
) -> MailSendResult<()> {
    let from = MailRecipient {
        address: settings.from_address.clone(),
        display_name: (!settings.from_name.is_empty()).then_some(settings.from_name.clone()),
    };
    tracing::debug!(
        from = %from.address,
        to = %to.address,
        subject = %rendered.subject,
        "mail: dispatching rendered message through configured sender"
    );

    mail_sender
        .send(MailMessage {
            from,
            to,
            subject: rendered.subject,
            text_body: rendered.text_body,
            html_body: rendered.html_body,
        })
        .await
}

fn validate_runtime_settings(settings: &MailRuntimeSettings) -> MailSendResult<()> {
    if !settings.is_configured() {
        return Err(MailDeliveryError::NotConfigured(
            "mail service is not configured".to_string(),
        ));
    }
    if !settings.is_ready_for_delivery() {
        return Err(MailDeliveryError::NotConfigured(
            "mail SMTP username and password must both be set or both be empty".to_string(),
        ));
    }
    Ok(())
}

/// Returns whether SMTP auth credentials should be attached to the transport.
///
/// This must use the same trim semantics as
/// [`MailRuntimeSettings::is_ready_for_delivery`]: a whitespace-only username
/// counts as "no auth" there, so attaching it here would burn outbox retry
/// budget on deliveries that can never authenticate.
fn smtp_auth_enabled(settings: &MailRuntimeSettings) -> bool {
    !settings.smtp_username.trim().is_empty()
}

fn build_transport(
    settings: &MailRuntimeSettings,
) -> MailSendResult<AsyncSmtpTransport<Tokio1Executor>> {
    tracing::debug!(
        smtp_host = %settings.smtp_host,
        smtp_port = settings.smtp_port,
        encryption_enabled = settings.encryption_enabled,
        auth_enabled = smtp_auth_enabled(settings),
        "mail: building SMTP transport"
    );
    let mut transport = if settings.encryption_enabled {
        if settings.smtp_port == 465 {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&settings.smtp_host)
                .map_err(|error| MailDeliveryError::Config(error.to_string()))?
                .port(settings.smtp_port)
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&settings.smtp_host)
                .map_err(|error| MailDeliveryError::Config(error.to_string()))?
                .port(settings.smtp_port)
        }
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&settings.smtp_host)
            .port(settings.smtp_port)
    };

    if smtp_auth_enabled(settings) {
        transport = transport.credentials(Credentials::new(
            settings.smtp_username.clone(),
            settings.smtp_password.clone(),
        ));
    }

    Ok(transport.build())
}

fn build_lettre_message(message: MailMessage) -> MailSendResult<Message> {
    let from = mailbox(message.from)?;
    let to = mailbox(message.to)?;

    Message::builder()
        .from(from)
        .to(to)
        .subject(message.subject)
        .multipart(
            MultiPart::alternative()
                .singlepart(SinglePart::plain(message.text_body))
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_HTML)
                        .body(message.html_body),
                ),
        )
        .map_err(|error| MailDeliveryError::Config(error.to_string()))
}

fn mailbox(recipient: MailRecipient) -> MailSendResult<Mailbox> {
    let address = recipient
        .address
        .parse::<Address>()
        .map_err(|error| MailDeliveryError::InvalidMessage(error.to_string()))?;
    Ok(Mailbox::new(recipient.display_name, address))
}

#[cfg(test)]
mod tests {
    use super::{MailDeliveryError, MemoryMailSender, SmtpMailSender, send_rendered_with};
    use crate::{
        DEFAULT_MAIL_SECURITY, DEFAULT_MAIL_SMTP_PORT, MailMessage, MailRecipient,
        MailRuntimeSettings, MailSender, RenderedMail,
    };

    fn settings() -> MailRuntimeSettings {
        MailRuntimeSettings {
            smtp_host: "smtp.example.com".to_string(),
            smtp_port: DEFAULT_MAIL_SMTP_PORT,
            smtp_username: String::new(),
            smtp_password: String::new(),
            from_address: "ops@example.com".to_string(),
            from_name: "Aster Ops".to_string(),
            encryption_enabled: DEFAULT_MAIL_SECURITY,
        }
    }

    #[tokio::test]
    async fn memory_sender_captures_messages() {
        let sender = MemoryMailSender::default();
        sender
            .send(MailMessage {
                from: MailRecipient {
                    address: "ops@example.com".to_string(),
                    display_name: None,
                },
                to: MailRecipient {
                    address: "user@example.com".to_string(),
                    display_name: None,
                },
                subject: "Hello".to_string(),
                text_body: "Hello".to_string(),
                html_body: "<p>Hello</p>".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(sender.messages().len(), 1);
        assert_eq!(sender.last_message().unwrap().subject, "Hello");
    }

    #[tokio::test]
    async fn send_rendered_with_uses_runtime_sender_identity() {
        let sender = crate::memory_sender();
        send_rendered_with(
            &sender,
            &settings(),
            MailRecipient {
                address: "user@example.com".to_string(),
                display_name: Some("User".to_string()),
            },
            RenderedMail {
                subject: "Subject".to_string(),
                text_body: "Text".to_string(),
                html_body: "<p>Text</p>".to_string(),
            },
        )
        .await
        .unwrap();

        let message = crate::memory_sender_ref(&sender)
            .and_then(MemoryMailSender::last_message)
            .unwrap();
        assert_eq!(message.from.address, "ops@example.com");
        assert_eq!(message.from.display_name.as_deref(), Some("Aster Ops"));
        assert_eq!(message.to.display_name.as_deref(), Some("User"));
    }

    #[test]
    fn smtp_auth_enabled_matches_readiness_trim_semantics() {
        let mut trimmed = settings();
        // Whitespace-only usernames count as "no auth" in readiness
        // validation (config.rs trims), so transport construction must not
        // attach credentials for them either.
        trimmed.smtp_username = " ".to_string();
        assert!(!super::smtp_auth_enabled(&trimmed));

        trimmed.smtp_username = " mailer ".to_string();
        assert!(super::smtp_auth_enabled(&trimmed));

        trimmed.smtp_username = String::new();
        assert!(!super::smtp_auth_enabled(&trimmed));
    }

    #[tokio::test]
    async fn smtp_sender_rejects_incomplete_settings_before_transport() {
        let sender = SmtpMailSender::new(|| MailRuntimeSettings {
            smtp_host: String::new(),
            smtp_port: DEFAULT_MAIL_SMTP_PORT,
            smtp_username: String::new(),
            smtp_password: String::new(),
            from_address: "ops@example.com".to_string(),
            from_name: String::new(),
            encryption_enabled: DEFAULT_MAIL_SECURITY,
        });

        let err = sender
            .send(MailMessage {
                from: MailRecipient {
                    address: "ops@example.com".to_string(),
                    display_name: None,
                },
                to: MailRecipient {
                    address: "user@example.com".to_string(),
                    display_name: None,
                },
                subject: "Subject".to_string(),
                text_body: "Text".to_string(),
                html_body: "<p>Text</p>".to_string(),
            })
            .await
            .unwrap_err();

        assert_eq!(
            err,
            MailDeliveryError::NotConfigured("mail service is not configured".to_string())
        );
    }
}
