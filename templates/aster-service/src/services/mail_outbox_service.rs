//! Product mail outbox boundary.
//!
//! Forge owns the shared outbox state machine. Product code should keep template payloads,
//! rendering, business URLs, and audit hooks here.

use crate::errors::Result;

const MAIL_OUTBOX_DISPATCH_CONFIG: aster_forge_mail::MailOutboxDispatchConfig =
    aster_forge_mail::MailOutboxDispatchConfig::new(
        20,
        60,
        32,
        aster_forge_mail::MailOutboxRetryPolicy::new(6, aster_forge_mail::DEFAULT_ERROR_MAX_LEN),
    );

/// Dispatches due mail outbox rows using the Forge DB-backed state machine.
#[allow(dead_code)]
pub async fn dispatch_due_with(
    db: &sea_orm::DatabaseConnection,
    mail_sender: &std::sync::Arc<dyn aster_forge_mail::MailSender>,
) -> Result<aster_forge_mail::DispatchStats> {
    let store = aster_forge_db::MailOutboxDbStore::new(db.clone());
    store
        .dispatch_due(
            &MAIL_OUTBOX_DISPATCH_CONFIG,
            |row| async move { deliver_one(mail_sender, &row).await },
            |context, attempt_count, subject| async move {
                tracing::info!(
                    mail_outbox_id = context.id,
                    attempt_count,
                    subject,
                    "mail sent"
                );
            },
            |context, attempt_count, error_message| async move {
                tracing::warn!(
                    mail_outbox_id = context.id,
                    attempt_count,
                    error = %error_message,
                    "mail delivery permanently failed"
                );
            },
        )
        .await
}

async fn deliver_one(
    mail_sender: &std::sync::Arc<dyn aster_forge_mail::MailSender>,
    row: &aster_forge_db::mail_outbox::Model,
) -> Result<String> {
    // Replace this placeholder with product template rendering.
    let subject = format!("{} notification", row.template_code.as_str());
    mail_sender
        .send(aster_forge_mail::MailMessage {
            from: aster_forge_mail::MailRecipient {
                address: "noreply@example.invalid".to_string(),
                display_name: Some(env!("CARGO_PKG_NAME").to_string()),
            },
            to: aster_forge_mail::MailRecipient {
                address: row.to_address.clone(),
                display_name: row.to_name.clone(),
            },
            subject: subject.clone(),
            text_body: row.payload_json.as_ref().to_string(),
            html_body: format!("<pre>{}</pre>", row.payload_json.as_ref()),
        })
        .await?;
    Ok(subject)
}

pub mod runtime {
    //! Mail outbox runtime component integration.

    /// Creates the mail outbox runtime component used by the product entrypoint.
    pub fn mail_runtime_component(
        state: &crate::runtime::AppState,
    ) -> aster_forge_runtime::RuntimeComponentBundleRegistration<
        aster_forge_runtime::ShutdownResourceComponent<MailOutboxRuntimeResources>,
    > {
        aster_forge_mail::mail_outbox_component(
            MailOutboxRuntimeResources {
                db: state.db_handles.writer().clone(),
                mail_sender: state.mail_sender.clone(),
            },
            |resources| async move {
                super::dispatch_due_with(&resources.db, &resources.mail_sender)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            },
        )
    }

    /// Minimal resources needed for shutdown drain.
    pub struct MailOutboxRuntimeResources {
        db: sea_orm::DatabaseConnection,
        mail_sender: std::sync::Arc<dyn aster_forge_mail::MailSender>,
    }
}
