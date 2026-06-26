//! Product-neutral mail outbox dispatch helpers.

use std::future::Future;
use std::time::Duration;

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
#[cfg(all(debug_assertions, feature = "openapi"))]
use utoipa::ToSchema;

/// Default maximum stored delivery error length.
pub const DEFAULT_ERROR_MAX_LEN: usize = 1024;

/// Default retry delays for persisting "sent" after successful delivery.
///
/// The first attempt is immediate. Later attempts provide a short best-effort
/// window for transient database failures after SMTP has already accepted the
/// message.
pub const DEFAULT_MARK_SENT_RETRY_DELAYS_MS: &[u64] = &[0, 100, 500, 2_000, 5_000];

/// Built-in Aster mail template code.
///
/// These are the shared account/auth mail templates used by current Aster
/// services. Products may still maintain their own template payload enum and
/// renderer; this type only standardizes the persisted template code for the
/// common catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(64))")]
#[serde(rename_all = "snake_case")]
pub enum MailTemplateCode {
    /// Account registration activation message.
    #[sea_orm(string_value = "register_activation")]
    RegisterActivation,
    /// Confirmation message for changing a contact address.
    #[sea_orm(string_value = "contact_change_confirmation")]
    ContactChangeConfirmation,
    /// Password reset message.
    #[sea_orm(string_value = "password_reset")]
    PasswordReset,
    /// Password reset notice message.
    #[sea_orm(string_value = "password_reset_notice")]
    PasswordResetNotice,
    /// Contact change notice message.
    #[sea_orm(string_value = "contact_change_notice")]
    ContactChangeNotice,
    /// External auth email verification message.
    #[sea_orm(string_value = "external_auth_email_verification")]
    ExternalAuthEmailVerification,
    /// Login email code message.
    #[sea_orm(string_value = "login_email_code")]
    LoginEmailCode,
    /// User invitation message.
    #[sea_orm(string_value = "user_invitation")]
    UserInvitation,
}

impl MailTemplateCode {
    /// Returns the stable persisted template code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RegisterActivation => "register_activation",
            Self::ContactChangeConfirmation => "contact_change_confirmation",
            Self::PasswordReset => "password_reset",
            Self::PasswordResetNotice => "password_reset_notice",
            Self::ContactChangeNotice => "contact_change_notice",
            Self::ExternalAuthEmailVerification => "external_auth_email_verification",
            Self::LoginEmailCode => "login_email_code",
            Self::UserInvitation => "user_invitation",
        }
    }
}

/// Raw JSON payload stored in `mail_outbox.payload_json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DeriveValueType)]
pub struct StoredMailPayload(pub String);

impl StoredMailPayload {
    /// Payload value persisted after terminal delivery, so sensitive template
    /// variables do not remain in the outbox row.
    pub const CLEARED_JSON: &str = "{}";

    /// Creates the cleared payload marker.
    pub fn cleared() -> Self {
        Self(Self::CLEARED_JSON.to_string())
    }
}

impl AsRef<str> for StoredMailPayload {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for StoredMailPayload {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<StoredMailPayload> for String {
    fn from(value: StoredMailPayload) -> Self {
        value.0
    }
}

/// Persistent mail outbox row status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(16))")]
#[serde(rename_all = "snake_case")]
pub enum MailOutboxStatus {
    /// Row is waiting for first delivery attempt.
    #[sea_orm(string_value = "pending")]
    Pending,
    /// Row is claimed by a dispatcher.
    #[sea_orm(string_value = "processing")]
    Processing,
    /// Row is waiting for another delivery attempt.
    #[sea_orm(string_value = "retry")]
    Retry,
    /// Row was delivered and marked sent.
    #[sea_orm(string_value = "sent")]
    Sent,
    /// Row exhausted retry policy.
    #[sea_orm(string_value = "failed")]
    Failed,
}

impl MailOutboxStatus {
    /// Returns whether the status is terminal.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Sent | Self::Failed)
    }
}

/// Aggregate counters returned by an outbox dispatch or drain pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DispatchStats {
    /// Rows claimed for processing.
    pub claimed: usize,
    /// Rows delivered and marked sent.
    pub sent: usize,
    /// Rows scheduled for retry.
    pub retried: usize,
    /// Rows permanently failed.
    pub failed: usize,
}

impl DispatchStats {
    /// Adds another counter set into this one.
    pub fn merge(&mut self, other: Self) {
        self.claimed += other.claimed;
        self.sent += other.sent;
        self.retried += other.retried;
        self.failed += other.failed;
    }

    /// Returns whether the dispatch pass did any visible work.
    pub const fn is_empty(self) -> bool {
        self.claimed == 0 && self.sent == 0 && self.retried == 0 && self.failed == 0
    }
}

/// Minimal row metadata needed by the shared outbox dispatcher.
///
/// Product crates keep their concrete database model and implement this trait on it so Forge can
/// log and apply retry policy without knowing SeaORM entities or product-specific columns.
pub trait MailOutboxDispatchRow: Clone {
    /// Stable row id.
    fn id(&self) -> i64;

    /// Current delivery attempt count stored on the row before this dispatch attempt.
    fn attempt_count(&self) -> i32;

    /// Stable product template code used for logs and audits.
    fn template_code(&self) -> &str;

    /// Recipient address used for logs and audits.
    fn to_address(&self) -> &str;
}

/// Configuration for the shared mail outbox dispatcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailOutboxDispatchConfig {
    /// Maximum rows loaded in one dispatch pass.
    pub batch_size: u64,
    /// Stale processing window in seconds. Product callbacks decide how this maps to timestamps.
    pub processing_stale_secs: i64,
    /// Maximum dispatch rounds during a drain pass.
    pub drain_max_rounds: usize,
    /// Delivery retry policy.
    pub retry_policy: MailOutboxRetryPolicy,
    /// Retry delays for marking a row as sent after delivery success.
    pub mark_sent_retry_delays_ms: &'static [u64],
}

impl MailOutboxDispatchConfig {
    /// Creates a dispatch config.
    pub const fn new(
        batch_size: u64,
        processing_stale_secs: i64,
        drain_max_rounds: usize,
        retry_policy: MailOutboxRetryPolicy,
    ) -> Self {
        Self {
            batch_size,
            processing_stale_secs,
            drain_max_rounds,
            retry_policy,
            mark_sent_retry_delays_ms: DEFAULT_MARK_SENT_RETRY_DELAYS_MS,
        }
    }
}

/// Retry and truncation policy for an outbox dispatcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailOutboxRetryPolicy {
    /// Maximum number of delivery attempts before permanent failure.
    pub max_attempts: i32,
    /// Maximum stored error string length, counted in Unicode scalar values.
    pub max_error_len: usize,
}

/// Decision returned after a mail delivery attempt fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailOutboxDeliveryFailureDecision {
    /// The row exhausted delivery attempts and should be moved to a terminal failure state.
    PermanentFailure {
        /// Attempt count that should be persisted on the outbox row.
        attempt_count: i32,
        /// Truncated delivery error safe for storage.
        error_message: String,
    },
    /// The row should be scheduled for another delivery attempt.
    Retry {
        /// Attempt count that should be persisted on the outbox row.
        attempt_count: i32,
        /// Delay before the next delivery attempt, in seconds.
        retry_delay_secs: i64,
        /// Truncated delivery error safe for storage.
        error_message: String,
    },
}

impl MailOutboxRetryPolicy {
    /// Creates a retry policy.
    pub const fn new(max_attempts: i32, max_error_len: usize) -> Self {
        Self {
            max_attempts,
            max_error_len,
        }
    }

    /// Returns whether `attempt_count` should permanently fail.
    pub const fn should_permanently_fail(&self, attempt_count: i32) -> bool {
        attempt_count >= self.max_attempts
    }

    /// Returns the delay before the next delivery retry.
    pub const fn retry_delay_secs(&self, attempt_count: i32) -> i64 {
        retry_delay_secs(attempt_count)
    }

    /// Truncates a delivery error according to this policy.
    pub fn truncate_error(&self, error: &str) -> String {
        truncate_error(error, self.max_error_len)
    }

    /// Classifies a failed delivery attempt.
    ///
    /// `attempt_count` should be the post-delivery attempt count that the
    /// product crate will persist on the outbox row.
    pub fn delivery_failure_decision(
        &self,
        attempt_count: i32,
        error: impl AsRef<str>,
    ) -> MailOutboxDeliveryFailureDecision {
        let error_message = self.truncate_error(error.as_ref());
        if self.should_permanently_fail(attempt_count) {
            MailOutboxDeliveryFailureDecision::PermanentFailure {
                attempt_count,
                error_message,
            }
        } else {
            MailOutboxDeliveryFailureDecision::Retry {
                attempt_count,
                retry_delay_secs: self.retry_delay_secs(attempt_count),
                error_message,
            }
        }
    }
}

/// Returns the default mail delivery retry delay for an attempt count.
pub const fn retry_delay_secs(attempt_count: i32) -> i64 {
    match attempt_count {
        1 => 5,
        2 => 15,
        3 => 60,
        4 => 300,
        5 => 900,
        _ => 1800,
    }
}

/// Truncates an error string without splitting UTF-8 code points.
pub fn truncate_error(error: &str, max_len: usize) -> String {
    error.chars().take(max_len).collect()
}

/// Runs one mail outbox dispatch pass using product-provided persistence callbacks.
///
/// Forge owns the control flow and retry classification. Product crates own time calculation,
/// repository calls, template rendering, mail auditing, and error types.
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_mail_outbox<
    R,
    E,
    List,
    ListFut,
    Claim,
    ClaimFut,
    Deliver,
    DeliverFut,
    MarkSent,
    MarkSentFut,
    MarkRetry,
    MarkRetryFut,
    MarkFailed,
    MarkFailedFut,
    OnSent,
    OnSentFut,
    OnFailed,
    OnFailedFut,
>(
    config: &MailOutboxDispatchConfig,
    mut list_claimable: List,
    mut try_claim: Claim,
    mut deliver: Deliver,
    mut mark_sent: MarkSent,
    mut mark_retry: MarkRetry,
    mut mark_failed: MarkFailed,
    mut on_sent: OnSent,
    mut on_failed: OnFailed,
) -> Result<DispatchStats, E>
where
    R: MailOutboxDispatchRow,
    E: std::fmt::Display,
    List: FnMut(u64, i64) -> ListFut,
    ListFut: Future<Output = Result<Vec<R>, E>>,
    Claim: FnMut(R) -> ClaimFut,
    ClaimFut: Future<Output = Result<bool, E>>,
    Deliver: FnMut(R) -> DeliverFut,
    DeliverFut: Future<Output = Result<String, E>>,
    MarkSent: FnMut(i64, usize) -> MarkSentFut,
    MarkSentFut: Future<Output = Result<bool, E>>,
    MarkRetry: FnMut(R, i32, i64, String) -> MarkRetryFut,
    MarkRetryFut: Future<Output = Result<bool, E>>,
    MarkFailed: FnMut(R, i32, String) -> MarkFailedFut,
    MarkFailedFut: Future<Output = Result<bool, E>>,
    OnSent: FnMut(R, i32, String) -> OnSentFut,
    OnSentFut: Future<Output = ()>,
    OnFailed: FnMut(R, i32, String) -> OnFailedFut,
    OnFailedFut: Future<Output = ()>,
{
    let rows = list_claimable(config.batch_size, config.processing_stale_secs).await?;
    let mut stats = DispatchStats::default();
    tracing::debug!(
        batch_size = config.batch_size,
        due_count = rows.len(),
        stale_secs = config.processing_stale_secs,
        "dispatching due mail outbox rows"
    );

    for row in rows {
        if !try_claim(row.clone()).await? {
            tracing::debug!(
                mail_outbox_id = row.id(),
                template_code = %row.template_code(),
                "mail outbox claim skipped because row was already claimed"
            );
            continue;
        }

        stats.claimed += 1;
        tracing::debug!(
            mail_outbox_id = row.id(),
            template_code = %row.template_code(),
            attempt_count = row.attempt_count(),
            "claimed mail outbox row"
        );

        match deliver(row.clone()).await {
            Ok(subject) => {
                tracing::debug!(
                    mail_outbox_id = row.id(),
                    template_code = %row.template_code(),
                    "mail outbox delivery succeeded"
                );
                match retry_mark_sent(row.id(), config.mark_sent_retry_delays_ms, &mut mark_sent)
                    .await
                {
                    Ok(true) => {
                        stats.sent += 1;
                        on_sent(row.clone(), row.attempt_count() + 1, subject).await;
                    }
                    Ok(false) => {
                        tracing::warn!(
                            mail_outbox_id = row.id(),
                            template_code = %row.template_code(),
                            to = %row.to_address(),
                            "mark_sent affected 0 rows after successful delivery; state will be rechecked"
                        );
                    }
                    Err(error) => {
                        tracing::error!(
                            mail_outbox_id = row.id(),
                            template_code = %row.template_code(),
                            to = %row.to_address(),
                            stale_secs = config.processing_stale_secs,
                            error = %error,
                            "CRITICAL: mail delivery succeeded but mark_sent failed after all retries; \
                             row remains Processing and may be re-claimed, causing duplicate delivery"
                        );
                    }
                }
            }
            Err(error) => {
                let attempt_count = row.attempt_count() + 1;
                match config
                    .retry_policy
                    .delivery_failure_decision(attempt_count, error.to_string())
                {
                    MailOutboxDeliveryFailureDecision::PermanentFailure {
                        attempt_count,
                        error_message,
                    } => {
                        if mark_failed(row.clone(), attempt_count, error_message.clone()).await? {
                            stats.failed += 1;
                            on_failed(row.clone(), attempt_count, error_message.clone()).await;
                        }
                        tracing::warn!(
                            mail_outbox_id = row.id(),
                            template_code = %row.template_code(),
                            to = %row.to_address(),
                            attempt_count,
                            error = %error_message,
                            "mail outbox delivery permanently failed"
                        );
                    }
                    MailOutboxDeliveryFailureDecision::Retry {
                        attempt_count,
                        retry_delay_secs,
                        error_message,
                    } => {
                        if mark_retry(
                            row.clone(),
                            attempt_count,
                            retry_delay_secs,
                            error_message.clone(),
                        )
                        .await?
                        {
                            stats.retried += 1;
                        }
                        tracing::warn!(
                            mail_outbox_id = row.id(),
                            template_code = %row.template_code(),
                            to = %row.to_address(),
                            attempt_count,
                            retry_delay_secs,
                            error = %error_message,
                            "mail outbox delivery failed; scheduled retry"
                        );
                    }
                }
            }
        }
    }

    tracing::debug!(
        claimed = stats.claimed,
        sent = stats.sent,
        retried = stats.retried,
        failed = stats.failed,
        "finished dispatching due mail outbox rows"
    );
    Ok(stats)
}

/// Runs mail outbox dispatch passes until no rows are claimed or the configured drain limit is hit.
#[allow(clippy::too_many_arguments)]
pub async fn drain_mail_outbox<E, Dispatch, DispatchFut>(
    config: &MailOutboxDispatchConfig,
    mut dispatch: Dispatch,
) -> Result<DispatchStats, E>
where
    E: std::fmt::Display,
    Dispatch: FnMut() -> DispatchFut,
    DispatchFut: Future<Output = Result<DispatchStats, E>>,
{
    let mut total = DispatchStats::default();
    tracing::debug!("draining mail outbox");

    for _ in 0..config.drain_max_rounds {
        let stats = dispatch().await?;
        let claimed = stats.claimed;
        total.merge(stats);
        if claimed == 0 {
            tracing::debug!("mail outbox drain finished because no rows were claimed");
            break;
        }
    }

    tracing::debug!(
        claimed = total.claimed,
        sent = total.sent,
        retried = total.retried,
        failed = total.failed,
        "mail outbox drain completed"
    );
    Ok(total)
}

/// Retries a product-provided `mark_sent` operation after SMTP success.
///
/// This helper exists to narrow the duplicate-delivery window where SMTP has
/// accepted a message but the database row still says `Processing`. The caller
/// provides the actual persistence function so repositories, transactions,
/// timestamps, and product errors stay in the product crate.
pub async fn retry_mark_sent<F, Fut, E>(
    id: i64,
    retry_delays_ms: &[u64],
    mut mark_sent: F,
) -> Result<bool, E>
where
    F: FnMut(i64, usize) -> Fut,
    Fut: Future<Output = Result<bool, E>>,
    E: std::fmt::Display,
{
    let mut last_err = None;
    for (index, delay_ms) in retry_delays_ms.iter().enumerate() {
        if *delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(*delay_ms)).await;
        }

        let attempt = index + 1;
        match mark_sent(id, attempt).await {
            Ok(updated) => {
                tracing::debug!(
                    mail_outbox_id = id,
                    attempt,
                    updated,
                    "marked mail outbox row as sent"
                );
                return Ok(updated);
            }
            Err(error) => {
                tracing::warn!(
                    mail_outbox_id = id,
                    attempt,
                    "mark_sent failed, will retry: {error}"
                );
                last_err = Some(error);
            }
        }
    }

    match last_err {
        Some(error) => Err(error),
        None => mark_sent(id, retry_delays_ms.len() + 1).await,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_ERROR_MAX_LEN, DispatchStats, MailOutboxRetryPolicy, MailOutboxStatus,
        MailTemplateCode, StoredMailPayload, retry_delay_secs, retry_mark_sent, truncate_error,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn dispatch_stats_merge_adds_all_counters() {
        let mut stats = DispatchStats {
            claimed: 1,
            sent: 2,
            retried: 3,
            failed: 4,
        };
        stats.merge(DispatchStats {
            claimed: 10,
            sent: 20,
            retried: 30,
            failed: 40,
        });

        assert_eq!(
            stats,
            DispatchStats {
                claimed: 11,
                sent: 22,
                retried: 33,
                failed: 44,
            }
        );
        assert!(!stats.is_empty());
        assert!(DispatchStats::default().is_empty());
    }

    #[test]
    fn retry_policy_matches_default_mail_backoff() {
        let policy = MailOutboxRetryPolicy::new(6, DEFAULT_ERROR_MAX_LEN);

        assert!(!policy.should_permanently_fail(5));
        assert!(policy.should_permanently_fail(6));
        assert_eq!(policy.retry_delay_secs(1), 5);
        assert_eq!(policy.retry_delay_secs(2), 15);
        assert_eq!(policy.retry_delay_secs(3), 60);
        assert_eq!(policy.retry_delay_secs(4), 300);
        assert_eq!(policy.retry_delay_secs(5), 900);
        assert_eq!(retry_delay_secs(99), 1800);
    }

    #[test]
    fn retry_policy_classifies_delivery_failures() {
        let policy = MailOutboxRetryPolicy::new(2, 3);

        assert_eq!(
            policy.delivery_failure_decision(1, "abcdef"),
            super::MailOutboxDeliveryFailureDecision::Retry {
                attempt_count: 1,
                retry_delay_secs: 5,
                error_message: "abc".to_string(),
            }
        );
        assert_eq!(
            policy.delivery_failure_decision(2, "abcdef"),
            super::MailOutboxDeliveryFailureDecision::PermanentFailure {
                attempt_count: 2,
                error_message: "abc".to_string(),
            }
        );
    }

    #[test]
    fn truncate_error_preserves_utf8_boundaries() {
        let value = "界".repeat(4);
        assert_eq!(truncate_error(&value, 3), "界界界");
    }

    #[test]
    fn mail_template_code_exposes_stable_storage_names() {
        assert_eq!(
            MailTemplateCode::RegisterActivation.as_str(),
            "register_activation"
        );
        assert_eq!(
            MailTemplateCode::ContactChangeConfirmation.as_str(),
            "contact_change_confirmation"
        );
        assert_eq!(MailTemplateCode::PasswordReset.as_str(), "password_reset");
        assert_eq!(
            MailTemplateCode::PasswordResetNotice.as_str(),
            "password_reset_notice"
        );
        assert_eq!(
            MailTemplateCode::ContactChangeNotice.as_str(),
            "contact_change_notice"
        );
        assert_eq!(
            MailTemplateCode::ExternalAuthEmailVerification.as_str(),
            "external_auth_email_verification"
        );
        assert_eq!(
            MailTemplateCode::LoginEmailCode.as_str(),
            "login_email_code"
        );
        assert_eq!(MailTemplateCode::UserInvitation.as_str(), "user_invitation");
    }

    #[test]
    fn mail_template_code_storage_names_fit_shared_schema() {
        let codes = [
            MailTemplateCode::RegisterActivation,
            MailTemplateCode::ContactChangeConfirmation,
            MailTemplateCode::PasswordReset,
            MailTemplateCode::PasswordResetNotice,
            MailTemplateCode::ContactChangeNotice,
            MailTemplateCode::ExternalAuthEmailVerification,
            MailTemplateCode::LoginEmailCode,
            MailTemplateCode::UserInvitation,
        ];

        for code in codes {
            assert!(
                code.as_str().len() <= 64,
                "mail template code `{}` exceeds shared schema length",
                code.as_str()
            );
        }
    }

    #[test]
    fn stored_mail_payload_helpers_preserve_raw_json() {
        let payload = StoredMailPayload::from("{\"token\":\"abc\"}".to_string());
        assert_eq!(payload.as_ref(), "{\"token\":\"abc\"}");

        let raw: String = payload.into();
        assert_eq!(raw, "{\"token\":\"abc\"}");
        assert_eq!(StoredMailPayload::cleared().as_ref(), "{}");
    }

    #[test]
    fn mail_outbox_status_terminal_states_are_explicit() {
        assert!(!MailOutboxStatus::Pending.is_terminal());
        assert!(!MailOutboxStatus::Processing.is_terminal());
        assert!(!MailOutboxStatus::Retry.is_terminal());
        assert!(MailOutboxStatus::Sent.is_terminal());
        assert!(MailOutboxStatus::Failed.is_terminal());
    }

    #[tokio::test]
    async fn retry_mark_sent_retries_until_success() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_closure = attempts.clone();

        let updated = retry_mark_sent(42, &[0, 0, 0], move |_id, _attempt| {
            let attempts = attempts_for_closure.clone();
            async move {
                let current = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                if current < 3 {
                    Err("temporary db error")
                } else {
                    Ok(true)
                }
            }
        })
        .await
        .expect("mark_sent should eventually succeed");

        assert!(updated);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_mark_sent_without_delays_runs_one_attempt() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_closure = attempts.clone();

        let error = retry_mark_sent(42, &[], move |_id, _attempt| {
            let attempts = attempts_for_closure.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err::<bool, _>("db down")
            }
        })
        .await
        .expect_err("mark_sent should fail");

        assert_eq!(error, "db down");
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
