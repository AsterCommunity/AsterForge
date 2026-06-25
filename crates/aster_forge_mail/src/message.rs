//! Product-neutral mail message models.
//!
//! These types describe the message envelope and rendered body passed between product mail
//! services, outbox workers, and test senders. They intentionally do not include product error
//! types, audit context, persistence metadata, or delivery state.

/// Address and optional display name for a mail sender or recipient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailRecipient {
    /// Email address used by the transport layer.
    pub address: String,
    /// Optional display name shown by mail clients.
    pub display_name: Option<String>,
}

/// Fully rendered mail message ready for a product-owned sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailMessage {
    /// Sender mailbox.
    pub from: MailRecipient,
    /// Recipient mailbox.
    pub to: MailRecipient,
    /// Rendered subject line.
    pub subject: String,
    /// Plain-text message body.
    pub text_body: String,
    /// HTML message body.
    pub html_body: String,
}
